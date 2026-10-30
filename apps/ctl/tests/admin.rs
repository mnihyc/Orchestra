//! `orchctl admin`: break-glass administration on the host: a reset link that works once, a system admin who is never the last to go, and a suspended person whose sessions end.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_api_contract::auth::{
    AuthLogin, AuthLoginBody, AuthRecoveryConfirm, AuthRecoveryConfirmBody, MeGet,
};
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::Reply;
use orch_db::queries::identity::{self, testing as rows};
use orch_testkit::clock;
use orch_types::{Id, Json, new_id};
use support::{Api, Deployment, admin, api_token, person};

const NEW_PASSWORD: &str = "a new password, long enough";

/// The secret in the address a reset link prints.
fn secret_of(url: &str) -> String {
    url.trim()
        .strip_prefix("https://orch.test/reset/")
        .unwrap_or_else(|| panic!("not an address of the system: {url}"))
        .to_owned()
}

async fn use_link(api: &Api, secret: &str, password: &str) -> Reply<()> {
    api.app
        .call(AuthRecoveryConfirm, &[])
        .body(&AuthRecoveryConfirmBody {
            token: secret.to_owned(),
            password: password.to_owned(),
        })
        .send()
        .await
}

async fn sign_in(api: &Api, login: &str, password: &str) -> u16 {
    api.app
        .call(AuthLogin, &[])
        .header("x-forwarded-for", "203.0.113.1")
        .header("user-agent", "tests/1.0")
        .body(&AuthLoginBody {
            login: login.to_owned(),
            password: Some(password.to_owned()),
            email_code: None,
            totp: None,
            recovery_code: None,
        })
        .send()
        .await
        .status
        .as_u16()
}

/// The lines of the audit trail, the oldest first: action, kind of actor,
/// actor, and what the line says it came from.
async fn trail(world: &Deployment) -> Vec<(String, String, Option<Id>, Option<String>)> {
    sqlx::query_as("SELECT action, actor_kind, actor_id, user_agent FROM audit_log ORDER BY at, id")
        .fetch_all(world.pool())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reset_link_prints_one_address_that_sets_a_new_password_once() {
    let world = Deployment::new().await;
    let ada = person(&world.db(), "ada").await;
    let api = Api::serve(&world.db()).await;

    let run = world.run(&["admin", "reset-link", "ada"]).await;

    let printed = run.succeeded();
    assert_eq!(
        printed.lines().count(),
        1,
        "the address, alone: {printed:?}"
    );
    let secret = secret_of(printed);
    assert!(run.err.contains("one use"), "{}", run.err);
    assert_eq!(use_link(&api, &secret, NEW_PASSWORD).await.status, 204);
    assert!(
        use_link(&api, &secret, "another password, long enough")
            .await
            .status
            .is_client_error(),
        "a link works once"
    );
    assert_eq!(sign_in(&api, "ada", NEW_PASSWORD).await, 200);
    let _ = ada;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_making_of_a_reset_link_is_in_the_audit_trail_with_the_actor_system_and_no_secret() {
    let world = Deployment::new().await;
    let ada = person(&world.db(), "ada").await;
    let run = world.run(&["admin", "reset-link", "ada"]).await;
    let secret = secret_of(run.succeeded());

    let lines = trail(&world).await;

    assert_eq!(lines.len(), 1);
    let (action, kind, actor, agent) = &lines[0];
    assert_eq!(action, "auth.password_reset");
    assert_eq!((kind.as_str(), *actor), ("system", None));
    assert!(
        agent
            .as_deref()
            .is_some_and(|agent| agent.starts_with("orchctl "))
    );
    let about: (Option<Id>, Json, String) =
        sqlx::query_as("SELECT object_id, details, summary FROM audit_log")
            .fetch_one(world.pool())
            .await
            .unwrap();
    assert_eq!(about.0, Some(ada));
    assert_eq!(about.1["stage"], "link_made");
    assert!(!about.1.to_string().contains(&secret) && !about.2.contains(&secret));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reset_link_made_for_somebody_who_lost_their_second_step_switches_it_off_when_used() {
    let world = Deployment::new().await;
    let lost = person(&world.db(), "lost").await;
    let kept = person(&world.db(), "kept").await;
    sqlx::query(
        "UPDATE users SET totp_secret = '\\x0102'::bytea, totp_enabled_at = now(), email_code_at = now()",
    )
    .execute(world.pool())
    .await
    .unwrap();
    let api = Api::serve(&world.db()).await;

    let clearing = world
        .run(&["admin", "reset-link", "lost", "--clear-second-step"])
        .await;
    let plain = world.run(&["admin", "reset-link", "kept"]).await;
    assert!(clearing.err.contains("second step"), "{}", clearing.err);
    use_link(&api, &secret_of(clearing.succeeded()), NEW_PASSWORD)
        .await
        .ok();
    use_link(&api, &secret_of(plain.succeeded()), NEW_PASSWORD)
        .await
        .ok();

    let mut conn = world.pool().acquire().await.unwrap();
    let lost = identity::user_by_id(&mut conn, lost)
        .await
        .unwrap()
        .unwrap();
    let kept = identity::user_by_id(&mut conn, kept)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !lost.totp_on() && !lost.email_code_on(),
        "both ways are off"
    );
    assert!(
        kept.totp_on() && kept.email_code_on(),
        "an ordinary link leaves them"
    );
    let cleared: Vec<(String, String, Option<Id>, Option<String>)> = trail(&world)
        .await
        .into_iter()
        .filter(|line| line.0 == "auth.second_step_cleared")
        .collect();
    assert_eq!(cleared.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_nobody_has_is_refused_in_one_line_and_leaves_nothing_behind() {
    let world = Deployment::new().await;
    person(&world.db(), "ada").await;

    for command in ["reset-link", "grant", "revoke", "suspend", "resume"] {
        let run = world.run(&["admin", command, "grace"]).await;
        run.refused_with("no user named grace");
    }
    // An address is no name: the name is what the person signs in by.
    world
        .run(&["admin", "suspend", "ada@example.org"])
        .await
        .refused_with("no user named ada@example.org");
    assert!(trail(&world).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_last_system_admin_who_can_sign_in_is_neither_made_an_ordinary_person_nor_suspended() {
    let world = Deployment::new().await;
    let boss = admin(&world.db(), "boss").await;
    person(&world.db(), "ada").await;

    world
        .run(&["admin", "revoke", "boss"])
        .await
        .refused_with("last system admin who can sign in");
    world
        .run(&["admin", "suspend", "boss"])
        .await
        .refused_with("last system admin who can sign in");
    let mut conn = world.pool().acquire().await.unwrap();
    let row = identity::user_by_id(&mut conn, boss)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.is_system_admin && !row.suspended(),
        "nothing was changed"
    );
    drop(conn);

    // With another one, the first may go; and then the other is the last.
    let granted = world.run(&["admin", "grant", "ada"]).await;
    assert!(granted.succeeded().contains("ada is a system admin now"));
    let revoked = world.run(&["admin", "revoke", "boss"]).await;
    assert!(
        revoked
            .succeeded()
            .contains("boss is no system admin any more")
    );
    world
        .run(&["admin", "revoke", "ada"])
        .await
        .refused_with("make another system admin first");
}

#[tokio::test(flavor = "multi_thread")]
async fn making_and_unmaking_a_system_admin_say_what_was_done_and_are_audited_as_the_system() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    person(&world.db(), "ada").await;

    let first = world.run(&["admin", "grant", "ada"]).await;
    let again = world.run(&["admin", "grant", "ada"]).await;
    let revoked = world.run(&["admin", "revoke", "ada"]).await;
    let ordinary = world.run(&["admin", "revoke", "ada"]).await;

    assert_eq!(first.succeeded(), "ada is a system admin now\n");
    assert!(again.succeeded().contains("already; nothing was changed"));
    assert_eq!(revoked.succeeded(), "ada is no system admin any more\n");
    assert!(ordinary.succeeded().contains("nothing was changed"));
    let lines: Vec<(String, String)> = trail(&world)
        .await
        .into_iter()
        .map(|line| (line.0, line.1))
        .collect();
    assert_eq!(
        lines,
        vec![
            ("user.admin_granted".to_owned(), "system".to_owned()),
            ("user.admin_revoked".to_owned(), "system".to_owned()),
        ],
        "an act that changed nothing leaves no line"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_suspended_persons_sessions_end_at_once_and_their_tokens_are_refused_until_it_is_lifted()
{
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    let ada = person(&world.db(), "ada").await;
    let access = new_token(ACCESS_TOKEN_PREFIX);
    {
        let mut conn = world.pool().acquire().await.unwrap();
        rows::insert_session(
            &mut conn,
            new_id(),
            ada,
            &token_hash(&access),
            clock::start() + Duration::days(30),
        )
        .await
        .unwrap();
    }
    let token = api_token(&world.db(), ada).await;
    let api = Api::serve(&world.db()).await;
    let me = |bearer: String| {
        let api = &api;
        async move {
            api.app
                .call(MeGet, &[])
                .token(&bearer)
                .send()
                .await
                .status
                .as_u16()
        }
    };
    assert_eq!(me(access.clone()).await, 200);
    assert_eq!(me(token.clone()).await, 200);
    let epoch_before = {
        let mut conn = world.pool().acquire().await.unwrap();
        orch_authz::epoch::current(&mut conn).await.unwrap()
    };

    let suspended = world.run(&["admin", "suspend", "ada"]).await;

    assert!(
        suspended.succeeded().contains("1 session ended"),
        "{}",
        suspended.out
    );
    assert_eq!(me(access.clone()).await, 401, "the session is over");
    assert_eq!(me(token.clone()).await, 401, "and so is the token");
    let live: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM sessions WHERE revoked_at IS NULL) + (SELECT count(*) FROM access_tokens)",
    )
    .fetch_one(world.pool())
    .await
    .unwrap();
    assert_eq!(live, 0, "no session and no access token is left to be used");
    let epoch_after = {
        let mut conn = world.pool().acquire().await.unwrap();
        orch_authz::epoch::current(&mut conn).await.unwrap()
    };
    assert_eq!(
        epoch_after,
        epoch_before + 1,
        "what was held open is looked at again"
    );

    let resumed = world.run(&["admin", "resume", "ada"]).await;
    assert!(resumed.succeeded().contains("can sign in again"));
    assert_eq!(me(token).await, 200, "a token of the person works again");
    assert_eq!(
        me(access).await,
        401,
        "a session that was ended is not brought back"
    );
    let kinds: Vec<String> = trail(&world).await.into_iter().map(|line| line.0).collect();
    assert_eq!(kinds, ["user.suspended", "user.resumed"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_options_may_come_after_the_command_and_a_deployment_that_is_not_there_is_told_in_one_line()
 {
    let world = Deployment::new().await;
    person(&world.db(), "ada").await;
    let after = {
        let mut command = world.command();
        command.env_remove("ORCH_CONFIG");
        let config = world.config();
        support::run(
            command,
            &[
                "admin",
                "reset-link",
                "ada",
                "--config",
                config.to_str().unwrap(),
            ],
        )
        .await
    };
    assert!(after.succeeded().starts_with("https://orch.test/reset/"));

    let mut missing = world.command();
    missing.env("ORCH_CONFIG", "/no/such/orch.toml");
    support::run(missing, &["admin", "suspend", "ada"])
        .await
        .refused_with("cannot read /no/such/orch.toml");

    let mut no_database = world.command();
    no_database.env("ORCH_DATABASE_URL", "");
    support::run(no_database, &["admin", "suspend", "ada"])
        .await
        .refused_with("ORCH_DATABASE_URL");

    let mut down = world.command();
    down.env(
        "ORCH_DATABASE_URL",
        "postgres://orch:hunter2@127.0.0.1:1/orch",
    );
    let refused = support::run(down, &["admin", "suspend", "ada"]).await;
    let line = refused.refused_with("cannot connect to the database");
    assert!(
        !line.contains("hunter2"),
        "a password is never printed: {line}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_database_that_has_no_schema_yet_is_told_to_wait_for_the_api() {
    let world = Deployment::new().await;
    let name = format!("orch_empty_{}", new_id().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(world.pool())
        .await
        .unwrap();
    let empty_url = world
        .test
        .url
        .rsplit_once('/')
        .map(|(server, _)| format!("{server}/{name}"))
        .unwrap();
    let mut command = world.command();
    command.env("ORCH_DATABASE_URL", &empty_url);

    let run = support::run(command, &["admin", "suspend", "ada"]).await;

    run.refused_with("start the API once");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE {name} WITH (FORCE)"
    )))
    .execute(world.pool())
    .await
    .unwrap();
}
