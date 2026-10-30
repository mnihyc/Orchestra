//! A small world on a real database for the tests of signing in: the application with every bound request, a clock that stands still, and people made without the cost of choosing a password.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use chrono::{DateTime, Duration, Utc};
use http::HeaderMap;
use orch_access_contract::pages::StoredKind;
use orch_api_contract::auth::{
    AuthLogin, AuthLoginBody, AuthResult, MeTotpEnable, MeTotpEnableBody, MeTotpSetup,
    MeTotpSetupBody,
};
use orch_api_contract::common::{CSRF_HEADER, Problem};
use orch_api_core::registry::Registry;
use orch_api_core::sign_in::codes::{self, TOTP_STEP_SECS};
use orch_api_core::sign_in::passwords;
use orch_api_core::testing::{Reply, TestApp};
use orch_config::ServerConfig;
use orch_db::Db;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::access::testing::add_entry;
use orch_db::queries::access::{PageKey, SYSTEM_ID};
use orch_db::queries::identity::{self, NewUser, testing as rows};
use orch_db::secrets::MasterKey;
use orch_settings::Target;
use orch_settings::write::{Write, write};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Json, new_id};
use sqlx::Postgres;
use sqlx::pool::PoolConnection;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The password every person of these tests has unless a test gives another:
/// long enough, and nothing like a name.
pub const PASSWORD: &str = "correct horse battery";

/// The address a stranger comes from.
pub const STRANGER: &str = "198.51.100.7";

/// The hash of a password, made once for the whole run of a test binary: a
/// hash is made to be slow, and a test that only needs a person to exist
/// should not wait for one.
async fn hashed(password: &str) -> String {
    static KEPT: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let kept = KEPT.get_or_init(Mutex::default);
    if let Some(hash) = kept.lock().unwrap().get(password) {
        return hash.clone();
    }
    let hash = passwords::hash(password).await.unwrap();
    kept.lock()
        .unwrap()
        .insert(password.to_owned(), hash.clone());
    hash
}

/// A person who was made for a test.
#[derive(Debug, Clone)]
pub struct Person {
    pub id: Id,
    pub username: String,
    pub email: String,
    pub personal_org: Id,
}

/// The application, its database and its clock.
pub struct World {
    pub app: TestApp,
    pub db: TestDb,
}

impl World {
    pub async fn new() -> Self {
        let mut config = ServerConfig::default();
        config.system.public_url = "https://orch.test".into();
        config.gateway.endpoints[0].url = "wss://gateway.test:9443".into();
        Self::with(config).await
    }

    /// A world whose wrong tries are locked after `limit` of them, so that a
    /// test of what a lock does does not have to make eight.
    pub async fn locking_after(limit: u32) -> Self {
        let mut config = ServerConfig::default();
        config.auth.login_failure_limit = limit;
        Self::with(config).await
    }

    pub async fn with(config: ServerConfig) -> Self {
        Self::with_parts(config, std::sync::Arc::default()).await
    }

    /// A world whose program registered these built-in sources of a key's facts.
    pub async fn with_builtin_sources(
        sources: std::sync::Arc<orch_provider_contract::builtins::BuiltinSources>,
    ) -> Self {
        Self::with_parts(ServerConfig::default(), sources).await
    }

    async fn with_parts(
        config: ServerConfig,
        sources: std::sync::Arc<orch_provider_contract::builtins::BuiltinSources>,
    ) -> Self {
        let db = TestDb::fresh().await;
        codes::install_master_key(MasterKey::new([7; 32]));
        // The system's defaults page as it ships, so that the roles hold what they do.
        {
            let mut conn = db.pool.acquire().await.unwrap();
            let page = PageKey::new("system", SYSTEM_ID, "defaults");
            for shipped in orch_access_contract::pages::shipped() {
                add_entry(
                    &mut conn,
                    &page,
                    shipped.profile.as_str(),
                    shipped.permission.as_str(),
                    ("role", None, Some(shipped.role.as_str())),
                    None,
                    clock::start(),
                )
                .await
                .unwrap();
            }
        }
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let app = TestApp::with_sources(
            Db::from_pool(db.pool.clone()),
            clock::manual(),
            registry,
            config,
            sources,
        );
        Self { app, db }
    }

    pub async fn conn(&self) -> PoolConnection<Postgres> {
        self.db.pool.acquire().await.unwrap()
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.app.app.clock.now()
    }

    pub fn advance(&self, by: Duration) {
        self.app.clock.advance(by);
    }

    /// Writes a setting of the system.
    pub async fn set(&self, key: &str, value: Json) {
        self.set_at(Target::System, key, value).await;
    }

    pub async fn set_at(&self, target: Target, key: &str, value: Json) {
        let mut conn = self.conn().await;
        write(
            &mut conn,
            &Write {
                actor: Actor::System,
                target,
                key,
                value,
                locked: None,
                expected_revision: None,
                may_lock: false,
                origin: &Origin::default(),
            },
            self.now(),
        )
        .await
        .unwrap();
    }

    /// Makes a person who has `PASSWORD`, a confirmed address
    /// `<username>@example.org`, and an org of their own.
    pub async fn person(&self, username: &str) -> Person {
        self.person_with(username, PASSWORD, true).await
    }

    pub async fn person_with(&self, username: &str, password: &str, confirmed: bool) -> Person {
        let hash = hashed(password).await;
        let id = new_id();
        let email = format!("{username}@example.org");
        let mut conn = self.conn().await;
        let made = identity::create_user(
            &mut conn,
            &NewUser {
                id,
                username,
                display_name: username,
                email: Some(&email),
                email_confirmed: confirmed,
                password_hash: Some(&hash),
                locale: "en",
                is_system_admin: false,
                created_by: None,
                personal_org: Some(orch_db::queries::identity::PersonalOrg {
                    closed: true,
                    flat: true,
                }),
            },
            self.now(),
        )
        .await
        .unwrap();
        // Their own org is theirs, as a person who registered has it.
        orch_authz::pages::start(
            &mut conn,
            StoredKind::Node,
            made.personal_org_id
                .expect("the fixture requests a personal org"),
            id,
            orch_types::Time::new(self.now()),
        )
        .await
        .unwrap();
        Person {
            id,
            username: username.to_owned(),
            email,
            personal_org: made
                .personal_org_id
                .expect("the fixture requests a personal org"),
        }
    }

    /// Switches on a mail server for the system.
    pub async fn mail_server(&self) {
        let by = self.person("mailman").await;
        let mut conn = self.conn().await;
        rows::insert_system_mail_channel(&mut conn, by.id)
            .await
            .unwrap();
    }

    pub async fn make_admin(&self, person: &Person) {
        let mut conn = self.conn().await;
        rows::make_admin(&mut conn, person.id).await.unwrap();
    }

    pub async fn suspend(&self, person: &Person) {
        let mut conn = self.conn().await;
        rows::suspend(&mut conn, person.id).await.unwrap();
    }

    /// What the outbox holds, the oldest first.
    pub async fn outbox(&self) -> Vec<Json> {
        let mut conn = self.conn().await;
        rows::outbox(&mut conn)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, payload)| payload)
            .collect()
    }

    /// The messages of one purpose that went to one address.
    pub async fn mails(&self, purpose: &str, to: &str) -> Vec<Json> {
        self.outbox()
            .await
            .into_iter()
            .filter(|row| row["purpose"] == purpose && row["to"] == to)
            .collect()
    }

    /// The keys of the audit rows, the oldest first.
    pub async fn audit(&self) -> Vec<String> {
        let mut conn = self.conn().await;
        rows::audit(&mut conn)
            .await
            .unwrap()
            .into_iter()
            .map(|(action, _, _)| action)
            .collect()
    }

    /// The names of the buckets of wrong tries.
    pub async fn buckets(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT bucket FROM login_throttle ORDER BY bucket")
            .fetch_all(&self.db.pool)
            .await
            .unwrap()
    }

    /// A sign-in with a password, from `ip`, and the cookies of the browser.
    pub async fn password_login(
        &self,
        login: &str,
        password: &str,
        ip: &str,
        cookies: &str,
    ) -> Reply<AuthResult> {
        self.attempt(
            AuthLoginBody {
                login: login.to_owned(),
                password: Some(password.to_owned()),
                email_code: None,
                totp: None,
                recovery_code: None,
            },
            ip,
            cookies,
        )
        .await
    }

    /// A sign-in with what is in `body`, from `ip`, in a browser that sends
    /// `cookies`.
    pub async fn attempt(&self, body: AuthLoginBody, ip: &str, cookies: &str) -> Reply<AuthResult> {
        let mut call = self
            .app
            .call(AuthLogin, &[])
            .peer(std::net::SocketAddr::new(ip.parse().unwrap(), 0))
            .header("user-agent", "tests/1.0");
        if !cookies.is_empty() {
            call = call.header("cookie", cookies);
        }
        call.body(&body).send().await
    }

    /// Signs the person in with their password from an address of their own.
    pub async fn sign_in(&self, person: &Person) -> Signed {
        let reply = self
            .password_login(&person.username, PASSWORD, "203.0.113.1", "")
            .await;
        Signed::of(reply)
    }

    /// Sets an authenticator up for a signed-in person and switches it on
    /// with a code of the clock's present. The secret is returned.
    pub async fn authenticator(&self, signed: &Signed) -> Vec<u8> {
        let reply = self
            .app
            .call(MeTotpSetup, &[])
            .token(&signed.result.access_token)
            .body(&MeTotpSetupBody {
                password: Some(PASSWORD.to_owned()),
                identity_proof: None,
            })
            .send()
            .await
            .ok();
        let secret = totp_rs::Secret::try_from_base32(&reply.secret)
            .unwrap()
            .as_bytes()
            .to_vec();
        let code = self.totp_code(&secret);
        let done = self
            .app
            .call(MeTotpEnable, &[])
            .token(&signed.result.access_token)
            .body(&MeTotpEnableBody { code })
            .send()
            .await;
        assert_eq!(done.status, 200, "{:?}", done.body);
        secret
    }

    /// The code an authenticator with `secret` shows at the clock's present.
    pub fn totp_code(&self, secret: &[u8]) -> String {
        self.totp_code_at(secret, self.now())
    }

    pub fn totp_code_at(&self, secret: &[u8], at: DateTime<Utc>) -> String {
        let totp = codes::authenticator(secret.to_vec(), "ada", "Orch").unwrap();
        totp.generate(u64::try_from(at.timestamp()).unwrap())
            .to_string()
    }

    /// The code that was last mailed to `to`, as it is shown.
    pub async fn mailed_code(&self, to: &str) -> String {
        self.mails("sign_in_code", to)
            .await
            .last()
            .map(|row| row["data"]["code"].as_str().unwrap().to_owned())
            .expect("a code was mailed")
    }
}

/// A person who is signed in: what they were given, and the cookies to send
/// back.
#[derive(Debug)]
pub struct Signed {
    pub result: AuthResult,
    pub session: String,
    pub device: String,
    pub headers: HeaderMap,
}

impl Signed {
    pub fn of(reply: Reply<AuthResult>) -> Self {
        let headers = reply.headers.clone();
        let status = reply.status;
        let result = match reply.body {
            Ok(result) => result,
            Err(problem) => panic!("not signed in: {status} {problem}"),
        };
        let found = cookies(&headers);
        let value = |name: &str| {
            found
                .iter()
                .find(|cookie| cookie.name == name)
                .map(|cookie| cookie.value.clone())
                .unwrap_or_default()
        };
        Self {
            result,
            session: value("orch_session"),
            device: value("orch_device"),
            headers,
        }
    }

    pub fn token(&self) -> &str {
        &self.result.access_token
    }

    /// The `Cookie` header of the browser that was given these.
    pub fn browser(&self) -> String {
        format!("orch_session={}; orch_device={}", self.session, self.device)
    }

    /// The `Cookie` header of a browser that has only the mark.
    pub fn mark_only(&self) -> String {
        format!("orch_device={}", self.device)
    }
}

/// A cookie that a response sets.
#[derive(Debug, Clone)]
pub struct SetCookie {
    pub name: String,
    pub value: String,
    pub attributes: Vec<String>,
}

impl SetCookie {
    pub fn has(&self, attribute: &str) -> bool {
        self.attributes
            .iter()
            .any(|each| each.eq_ignore_ascii_case(attribute))
    }

    pub fn max_age(&self) -> Option<i64> {
        self.attributes
            .iter()
            .find_map(|each| each.strip_prefix("Max-Age="))
            .and_then(|seconds| seconds.parse().ok())
    }

    pub fn path(&self) -> Option<&str> {
        self.attributes
            .iter()
            .find_map(|each| each.strip_prefix("Path="))
    }
}

pub fn cookies(headers: &HeaderMap) -> Vec<SetCookie> {
    headers
        .get_all("set-cookie")
        .iter()
        .map(|value| {
            let text = value.to_str().unwrap();
            let mut parts = text.split("; ");
            let (name, value) = parts.next().unwrap().split_once('=').unwrap();
            SetCookie {
                name: name.to_owned(),
                value: value.to_owned(),
                attributes: parts.map(str::to_owned).collect(),
            }
        })
        .collect()
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<SetCookie> {
    cookies(headers)
        .into_iter()
        .find(|cookie| cookie.name == name)
}

/// What a refusal says, apart from the identifier of the request: for
/// answers that have to be the same.
pub fn told(problem: &Problem) -> Json {
    let mut shown = serde_json::to_value(problem).unwrap();
    shown.as_object_mut().unwrap().remove("request_id");
    shown
}

/// The header a request that is made by the cookie alone needs.
pub fn csrf() -> (&'static str, &'static str) {
    (CSRF_HEADER, "1")
}

/// How many seconds a code lasts.
pub fn step() -> Duration {
    Duration::seconds(i64::try_from(TOTP_STEP_SECS).unwrap())
}

/// The time the clock of the tests starts at.
pub fn start() -> DateTime<Utc> {
    clock::start()
}

// ---- counting statements ----

/// Counts the statements that run on this thread: sqlx tells of each one as an
/// event. One subscriber is installed for the whole test binary, and it counts
/// per thread, so that tests that run side by side count their own.
pub struct Statements;

thread_local! {
    static COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Statements {
    /// Makes sure the subscriber is installed, and returns the count so far
    /// on this thread.
    pub fn now() -> usize {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = tracing::subscriber::set_global_default(Statements);
        });
        COUNT.with(std::cell::Cell::get)
    }
}

impl tracing::Subscriber for Statements {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        static PLANS: OnceLock<bool> = OnceLock::new();
        metadata.target() == "sqlx::query"
            || (*PLANS.get_or_init(|| std::env::var_os("ORCH_TEST_SQL_PLANS").is_some())
                && metadata.target().starts_with("sqlx"))
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() != "sqlx::query" {
            // An opt-in fresh database can send its ANALYZE/BUFFERS plans as notices.
            // Capture the actual executed statements without reproducing their bindings.
            struct Notice(Vec<String>);
            impl tracing::field::Visit for Notice {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0.push(format!("{}={value:?}", field.name()));
                }
            }
            let mut notice = Notice(Vec::new());
            event.record(&mut notice);
            eprintln!("SQL notice {}", notice.0.join(" "));
        }
        if event.metadata().target() == "sqlx::query" {
            COUNT.with(|count| count.set(count.get() + 1));
            static TIMINGS: OnceLock<bool> = OnceLock::new();
            if *TIMINGS.get_or_init(|| std::env::var_os("ORCH_TEST_SQL_TIMINGS").is_some()) {
                // SQL summaries and durations identify a slow read without logging bound values.
                struct Timings(Vec<String>);
                impl tracing::field::Visit for Timings {
                    fn record_debug(
                        &mut self,
                        field: &tracing::field::Field,
                        value: &dyn std::fmt::Debug,
                    ) {
                        if matches!(field.name(), "summary" | "elapsed" | "elapsed_secs") {
                            self.0.push(format!("{}={value:?}", field.name()));
                        }
                    }
                }
                let mut timings = Timings(Vec::new());
                event.record(&mut timings);
                eprintln!("SQL timing {}", timings.0.join(" "));
            }
        }
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}
