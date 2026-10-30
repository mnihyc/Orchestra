//! What every request goes through around its handler, shown on endpoints declared for this test alone.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::Duration;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::routes::Access::{Cookie, Public, Session, User};
use orch_api_core::middleware::authentication::{
    ACCESS_TOKEN_PREFIX, API_TOKEN_PREFIX, new_token, token_hash,
};
use orch_api_core::middleware::requests::{ApiError, ApiResult};
use orch_api_core::registry::{Answer, Ctx, Registry};
use orch_api_core::testing::TestApp;
use orch_db::Db;
use orch_testkit::{clock, database::TestDb};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EchoBody {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    times: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EchoReply {
    text: String,
    by: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShoutQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loud: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    kind: Vec<String>,
}

orch_api_contract::endpoints! {
    /// Says which kinds were asked for.
    KindList: GET "/kinds", Public, query ListQuery, none, json EchoReply, 200;
    /// Says the text back, or nothing when none was sent.
    EchoOptional: POST "/echo-optional", Public, none, json OptionalBody, json EchoReply, 200;
    /// Says the text back.
    EchoCreate: POST "/echo", Public, none, json EchoBody, json EchoReply, 201;
    /// Says who is calling.
    WhoGet: GET "/who/{id}", User, query ShoutQuery, none, json EchoReply, 200;
    /// Done in a browser only.
    BrowserOnly: POST "/browser-only", Session, none, none, none, 204;
    /// Done by the cookie alone.
    CookieOnly: POST "/cookie-only", Cookie, none, none, none, 204;
}

async fn echo(_: Ctx, (): (), body: EchoBody) -> ApiResult<Answer<EchoReply>> {
    if body.text == "boom" {
        return Err(ApiError::internal("disk on fire at rack 7"));
    }
    let text = body.text.repeat(body.times.unwrap_or(1) as usize);
    Ok(Answer::from(EchoReply { text, by: None }).header(http::header::SET_COOKIE, "seen=1"))
}

async fn who(ctx: Ctx, query: ShoutQuery, (): ()) -> ApiResult<EchoReply> {
    let name = ctx.user()?.user.username.clone();
    let text = ctx.id("id")?.to_string();
    Ok(EchoReply {
        text,
        by: Some(if query.loud == Some(true) {
            name.to_uppercase()
        } else {
            name
        }),
    })
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionalBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

async fn kinds(_: Ctx, query: ListQuery, (): ()) -> ApiResult<EchoReply> {
    Ok(EchoReply {
        text: query.kind.join("+"),
        by: None,
    })
}

async fn echo_optional(_: Ctx, (): (), body: OptionalBody) -> ApiResult<EchoReply> {
    Ok(EchoReply {
        text: body.text.unwrap_or_else(|| "nothing".to_owned()),
        by: None,
    })
}

async fn nothing(_: Ctx, (): (), (): ()) -> ApiResult<()> {
    Ok(())
}

/// A change by a signed-in caller asks for its authority again in the transaction that would
/// write, as every such request does, and then changes nothing.
async fn checked(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (tx, _guard) = orch_api_core::middleware::transaction_guard::begin(&ctx).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

struct World {
    app: TestApp,
    _db: TestDb,
    session_token: String,
    api_token: String,
    user: orch_types::Id,
}

async fn world() -> World {
    let db = TestDb::fresh().await;
    let clock = clock::manual();
    let now = clock::start();
    let user = orch_types::new_id();
    let session = orch_types::new_id();
    let (session_token, api_token) = (new_token(ACCESS_TOKEN_PREFIX), new_token(API_TOKEN_PREFIX));
    sqlx_setup(&db, user, session, &session_token, &api_token, now).await;
    let mut registry = Registry::new();
    registry
        .on(EchoCreate, echo)
        .on(WhoGet, who)
        .on(BrowserOnly, checked)
        .on(CookieOnly, nothing)
        .on(KindList, kinds)
        .on(EchoOptional, echo_optional);
    let app = TestApp::new(Db::from_pool(db.pool.clone()), clock, registry);
    World {
        app,
        _db: db,
        session_token,
        api_token,
        user,
    }
}

async fn sqlx_setup(
    db: &TestDb,
    user: orch_types::Id,
    session: orch_types::Id,
    access: &str,
    api: &str,
    now: chrono::DateTime<chrono::Utc>,
) {
    let mut conn = db.pool.acquire().await.unwrap();
    orch_db::queries::identity::testing::insert_user(&mut conn, user, "ada")
        .await
        .unwrap();
    orch_db::queries::identity::testing::insert_session(
        &mut conn,
        session,
        user,
        &token_hash(access),
        now + Duration::minutes(15),
    )
    .await
    .unwrap();
    orch_db::queries::identity::testing::insert_api_token(&mut conn, user, &token_hash(api))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_declared_request_answers_with_its_declared_status_and_reply() {
    let world = world().await;
    let reply = world
        .app
        .call(EchoCreate, &[])
        .body(&EchoBody {
            text: "ab".into(),
            times: Some(2),
        })
        .send()
        .await;
    assert_eq!(reply.status, 201);
    assert!(reply.headers.contains_key("x-request-id"));
    assert_eq!(reply.headers["set-cookie"], "seen=1");
    assert_eq!(reply.ok().text, "abab");
}

#[tokio::test]
async fn a_member_nobody_defined_and_a_wrong_type_are_refused_with_where_they_are() {
    let world = world().await;
    let unknown = world
        .app
        .call(EchoCreate, &[])
        .raw_body(&serde_json::json!({ "text": "a", "extra": 1 }))
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
    // The detail is the code's sentence; the fault says which member nobody defined.
    let fault = unknown.extra["errors"][0]["message"].as_str().unwrap();
    assert!(fault.contains("extra"), "{fault}");
    let wrong = world
        .app
        .call(EchoCreate, &[])
        .raw_body(&serde_json::json!({ "text": "a", "times": "two" }))
        .send()
        .await
        .problem();
    assert_eq!(wrong.extra["errors"][0]["path"], "times");
    let query = world
        .app
        .call(WhoGet, &[&world.user])
        .token(&world.session_token)
        .raw_query("volume=11")
        .send()
        .await
        .problem();
    assert_eq!(query.code, ErrorCode::RequestInvalid);
    let stray = world
        .app
        .call(EchoCreate, &[])
        .raw_query("x=1")
        .body(&EchoBody {
            text: "a".into(),
            times: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(stray.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_caller_is_known_by_a_session_token_or_an_api_token_and_nobody_without() {
    let world = world().await;
    let none = world
        .app
        .call(WhoGet, &[&world.user])
        .send()
        .await
        .problem();
    assert_eq!((none.code, none.status), (ErrorCode::AuthRequired, 401));
    let unknown = world
        .app
        .call(WhoGet, &[&world.user])
        .token("oat_nope")
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::AuthInvalidToken);

    let by_session = world
        .app
        .call(WhoGet, &[&world.user])
        .token(&world.session_token)
        .query(&ShoutQuery { loud: Some(true) })
        .send()
        .await
        .ok();
    assert_eq!(
        (by_session.by.as_deref(), by_session.text),
        (Some("ADA"), world.user.to_string())
    );
    let by_token = world
        .app
        .call(WhoGet, &[&world.user])
        .token(&world.api_token)
        .send()
        .await
        .ok();
    assert_eq!(by_token.by.as_deref(), Some("ada"));
    // Something that is no identifier names nothing.
    let nothing_there = world
        .app
        .call(WhoGet, &[&"not-an-id"])
        .token(&world.session_token)
        .send()
        .await
        .problem();
    assert_eq!(nothing_there.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn an_access_token_stops_working_when_it_runs_out_and_when_its_user_is_suspended() {
    let world = world().await;
    world.app.clock.advance(Duration::minutes(16));
    let expired = world
        .app
        .call(WhoGet, &[&world.user])
        .token(&world.session_token)
        .send()
        .await
        .problem();
    assert_eq!(expired.code, ErrorCode::AuthInvalidToken);

    let mut conn = world.app.app.db.acquire().await.unwrap();
    orch_db::queries::identity::testing::suspend(&mut conn, world.user)
        .await
        .unwrap();
    let suspended = world
        .app
        .call(WhoGet, &[&world.user])
        .token(&world.api_token)
        .send()
        .await
        .problem();
    assert_eq!(
        (suspended.code, suspended.status),
        (ErrorCode::AuthSuspended, 401)
    );
}

#[tokio::test]
async fn what_is_done_in_a_browser_refuses_a_token_and_a_cookie_request_needs_its_header() {
    let world = world().await;
    let by_token = world
        .app
        .call(BrowserOnly, &[])
        .token(&world.api_token)
        .send()
        .await
        .problem();
    assert_eq!(by_token.code, ErrorCode::AuthSessionRequired);
    assert_eq!(
        world
            .app
            .call(BrowserOnly, &[])
            .token(&world.session_token)
            .send()
            .await
            .status,
        204
    );

    let without = world.app.call(CookieOnly, &[]).send().await.problem();
    assert_eq!(without.code, ErrorCode::AuthCsrf);
    assert_eq!(
        world
            .app
            .call(CookieOnly, &[])
            .cookie("__Host-orch_session", "ors_x")
            .send()
            .await
            .status,
        204
    );
}

#[tokio::test]
async fn an_unforeseen_failure_tells_the_caller_nothing_of_its_cause() {
    let world = world().await;
    let problem = world
        .app
        .call(EchoCreate, &[])
        .body(&EchoBody {
            text: "boom".into(),
            times: None,
        })
        .send()
        .await
        .problem();
    assert_eq!((problem.code, problem.status), (ErrorCode::Internal, 500));
    assert!(!problem.detail.contains("rack 7"));
    assert!(!problem.request_id.is_empty());
}

#[tokio::test]
async fn a_parameter_given_several_times_is_a_list_and_no_body_is_a_body_with_no_members() {
    let world = world().await;
    let several = world
        .app
        .call(KindList, &[])
        .raw_query("kind=a&kind=b")
        .send()
        .await
        .ok();
    assert_eq!(several.text, "a+b");
    assert_eq!(
        world
            .app
            .call(KindList, &[])
            .raw_query("kind=a")
            .send()
            .await
            .ok()
            .text,
        "a"
    );
    assert_eq!(world.app.call(KindList, &[]).send().await.ok().text, "");

    assert_eq!(
        world.app.call(EchoOptional, &[]).send().await.ok().text,
        "nothing"
    );
    // A request that needs a member says which in its faults, also when nothing at all was sent; the
    // detail is the catalogue's sentence for the code.
    let missing = world.app.call(EchoCreate, &[]).send().await.problem();
    assert_eq!(missing.code, ErrorCode::RequestInvalid);
    let said = missing.extra["errors"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(said.contains("text"), "{:?}", missing.extra);
}
