//! Native sign-in methods at the credential keeper, admitted by actual home ownership and its machine policy.
//!
//! A login is the provider's: it is given on the provider and lives in its runtime, and no environment is
//! ever signed in to anything. What a person gives for a secret field is sealed the moment the request has
//! it, in a row of its own, and is never logged, never returned and never kept in a form anybody can read:
//! the command that gives it to the harness refers to the row, the link seals it to the machine of the
//! runtime when the frame is written, and the row is deleted once the harness has it. A sign-in whose
//! runtime is not up waits for it, with its secrets sealed, and is forgotten if the runtime is not made.
//! What the harness shows (an address, a code, a command) and asks for comes back as data, step by step,
//! and the page renders it without knowing which harness it is.

use crate::middleware::authentication::Via;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::resources::providers::show;
use crate::routes::resources::sealing::{SECRET_MAX_BYTES, put_provider_secret};
use crate::routes::resources::{Home, begin_mutation, record};
use orch_access_contract::audit::AuditAction;
use orch_agent_contract::login::{LoginKind, LoginMethodDef};
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_api_contract::resources::{
    self as api, LoginProgress, LoginState, Provider, ProviderLoginInputBody,
    ProviderLoginStartBody,
};
use orch_db::DbError;
use orch_db::queries::resources::provider_runtime::{self as rows, LOGIN_SECRET_PREFIX, State};
use orch_db::queries::resources::thread_engine as agent_rows;
use orch_engine::provider_setup::{Asked, Delivered, Pending, SignInValue};
use orch_types::fields::validate;
use orch_types::{Field, FieldType, Id, Json};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

/// The most bytes of a text, secret or secret text that a field takes when its method states no limit:
/// what the specification gives a field, which holds any token or key.
const FIELD_BYTES_DEFAULT: usize = 4_096;
/// How long a request that imports a credential waits for the harness to say how it went: an import is
/// done in one request, and its answer carries the end of it, or the reason it came to none.
const IMPORT_WAIT: Duration = Duration::from_secs(15);
/// How long a request that answers a sign-in waits for the harness to move on from the question.
const ANSWER_WAIT: Duration = Duration::from_secs(5);
/// How often the state of a sign-in is looked at while one of those requests waits.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::ProviderLoginStart, start)
        .on(api::ProviderLoginInput, input)
        .on(api::ProviderLoginGet, get)
        .on(api::ProviderLoginCancel, cancel)
        .on(api::ProviderLogout, logout);
}

// ---------------------------------------------------------------- what a person typed, and as what

fn fault(path: &str, message: &str) -> FieldFault {
    FieldFault {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

/// What a person typed for the fields of a method or of a question, as the typed values the adapter
/// takes them in: a boolean from `true` or `false`, a map from a JSON object of texts, anything else as
/// the text it is. Every fault is returned, named by the field.
fn typed(
    fields: &[Field],
    given: &BTreeMap<String, String>,
    prefix: &str,
) -> Result<BTreeMap<String, Json>, Vec<FieldFault>> {
    let mut values: BTreeMap<String, Json> = BTreeMap::new();
    let mut faults: Vec<FieldFault> = Vec::new();
    for (name, text) in given {
        let path = format!("{prefix}{name}");
        let Some(field) = fields.iter().find(|field| &field.name == name) else {
            faults.push(fault(&path, "is not asked for"));
            continue;
        };
        if text.len() > SECRET_MAX_BYTES {
            faults.push(fault(&path, "is too long"));
            continue;
        }
        let limit = field
            .max_bytes
            .map(|most| usize::try_from(most).unwrap_or(usize::MAX));
        if limit.is_none()
            && matches!(
                field.kind,
                FieldType::Text | FieldType::Secret | FieldType::SecretText
            )
            && text.len() > FIELD_BYTES_DEFAULT
        {
            faults.push(fault(
                &path,
                &format!("must be at most {FIELD_BYTES_DEFAULT} bytes"),
            ));
            continue;
        }
        let value = match field.kind {
            FieldType::Boolean => match text.as_str() {
                "true" => Json::Bool(true),
                "false" => Json::Bool(false),
                _ => {
                    faults.push(fault(&path, "must be true or false"));
                    continue;
                }
            },
            FieldType::Map | FieldType::SecretMap => match serde_json::from_str::<Json>(text) {
                Ok(object @ Json::Object(_)) => object,
                _ => {
                    faults.push(fault(
                        &path,
                        "must be names with values, written as a JSON object",
                    ));
                    continue;
                }
            },
            _ => Json::String(text.clone()),
        };
        values.insert(name.clone(), value);
    }
    if faults.is_empty() {
        Ok(values)
    } else {
        Err(faults)
    }
}

/// The values a request gave, checked against the fields they were given for.
fn checked(
    fields: &[Field],
    given: &BTreeMap<String, String>,
    prefix: &str,
) -> ApiResult<BTreeMap<String, Json>> {
    let values =
        typed(fields, given, prefix).map_err(|faults| ApiError::from(Problem::invalid(faults)))?;
    validate(fields, &Json::Object(values.clone().into_iter().collect())).map_err(|faults| {
        ApiError::from(Problem::invalid(
            faults
                .into_iter()
                .map(|each| fault(&format!("{prefix}{}", each.path), &each.message))
                .collect(),
        ))
    })?;
    Ok(values)
}

// ---------------------------------------------------------------- the state

/// The state of a provider's sign-in as the API shows it: what the harness last told, else none running.
fn shown(state: Option<&Json>) -> LoginState {
    state
        .and_then(|told| serde_json::from_value::<LoginState>(told.clone()).ok())
        .unwrap_or(LoginState {
            state: LoginProgress::Idle,
            method: None,
            shows: Vec::new(),
            asks: None,
            expires_at: None,
            error: None,
        })
}

/// The provider, which has to be a native subscription. Reading and cancellation need no sign-in grant.
async fn signing_in_provider(ctx: &Ctx, id: Id) -> ApiResult<State> {
    let mut conn = ctx.app.db.acquire().await?;
    let state = rows::state(&mut conn, id)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    subscription(&state)?;
    Ok(state)
}

fn subscription(state: &State) -> ApiResult<()> {
    if state.kind != "subscription" {
        return Err(ApiError::new(
            ErrorCode::RequestInvalid,
            "only a subscription is signed in: an API provider has a key",
        ));
    }
    Ok(())
}

async fn admit(conn: &mut orch_db::Conn, id: Id) -> ApiResult<()> {
    if !orch_settings::provider_sign_in(conn, id)
        .await
        .map_err(ApiError::internal)?
        .allowed
    {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "provider.signin: this selected harness home belongs to its machine's owner; the credential keeper has not permitted Orch to change or renew its sign-in",
        ));
    }
    Ok(())
}

async fn current(ctx: &Ctx, id: Id) -> ApiResult<LoginState> {
    let mut conn = ctx.app.db.acquire().await?;
    let state = rows::state(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(shown(state.login_state.as_ref()))
}

/// The ways of signing in the provider's harness declares, when the harness has said what it is.
async fn methods_of(
    conn: &mut orch_db::Conn,
    state: &State,
) -> ApiResult<Option<Vec<LoginMethodDef>>> {
    let Some(agent) = state.agent_id else {
        return Ok(None);
    };
    let Some(profile) = agent_rows::agent(conn, agent).await? else {
        return Ok(None);
    };
    Ok(orch_engine::harnesses::descriptor_of(&profile)
        .map_err(ApiError::internal)?
        .and_then(|descriptor| descriptor.native_provider)
        .map(|native| native.login_methods))
}

/// Waits until the sign-in has ended or has asked for something, for as long as `how_long`, and gives
/// the state it stands in.
async fn settled(
    ctx: &Ctx,
    id: Id,
    how_long: Duration,
    from: &LoginState,
) -> ApiResult<LoginState> {
    let until = tokio::time::Instant::now() + how_long;
    loop {
        let now = current(ctx, id).await?;
        let moved = now != *from;
        let done = matches!(
            now.state,
            LoginProgress::Completed | LoginProgress::Failed | LoginProgress::Expired
        );
        if done || (moved && now.state == LoginProgress::AwaitingUser) {
            return Ok(now);
        }
        if tokio::time::Instant::now() >= until {
            return Ok(now);
        }
        tokio::time::sleep(LOOK_EVERY).await;
    }
}

// ---------------------------------------------------------------- starting one

/// Checks a request to sign in against the ways the provider's harness declares, where it has said what
/// it declares: the method is one of them and what was given fits its fields, every fault named by
/// its field. A harness that has not said what it is cannot be asked, and nothing is judged.
pub(crate) fn check_request(
    known: Option<&Vec<LoginMethodDef>>,
    body: &ProviderLoginStartBody,
) -> ApiResult<Option<LoginMethodDef>> {
    let given = body.values.clone().unwrap_or_default();
    let method = match known {
        Some(methods) if !methods.is_empty() => Some(
            methods
                .iter()
                .find(|method| method.id == body.method)
                .ok_or_else(|| {
                    ApiError::from(Problem::invalid(vec![fault(
                        "method",
                        "is no way of signing in that this provider's harness has",
                    )]))
                })?
                .clone(),
        ),
        _ => None,
    };
    if let Some(method) = &method {
        checked(&method.fields, &given, "values.")?;
    }
    Ok(method)
}

/// Begins a sign-in on a provider by `body.method`: the values are checked against the method's fields
/// where the harness has said what it declares, the secrets among them are sealed at once, the sign-in is
/// kept for the runtime and given to its harness, and the answer is where it stands. A request that
/// imports a credential waits for the harness to say how it went.
pub(crate) async fn begin(
    ctx: &Ctx,
    id: Id,
    body: &ProviderLoginStartBody,
) -> ApiResult<LoginState> {
    let (current_ctx, mut tx, _) = begin_mutation(ctx).await?;
    let ctx = &current_ctx;
    let by = ctx.user()?.id();
    let state = rows::lock(&mut tx, id)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    subscription(&state)?;
    admit(&mut tx, id).await?;
    let known = methods_of(&mut tx, &state).await?;
    let given = body.values.clone().unwrap_or_default();
    let method = check_request(known.as_ref(), body)?;
    if matches!(&ctx.user()?.via, Via::Token { .. })
        && method
            .as_ref()
            .is_none_or(|method| method.kind != LoginKind::Import)
    {
        return Err(Problem::invalid(vec![fault(
            "method",
            "use a method that imports a credential for a script; open the provider's Sign in page for an interactive method",
        )]).into());
    }
    // What waited before is replaced, with its secrets.
    rows::delete_login_secrets(&mut tx, id, LOGIN_SECRET_PREFIX).await?;
    let secret_fields: Vec<String> = match &method {
        Some(method) => method
            .fields
            .iter()
            .filter(|field| field.kind.is_secret())
            .map(|field| field.name.clone())
            .collect(),
        // A harness that has not said what it declares: nothing is known to be no secret.
        None => given.keys().cloned().collect(),
    };
    let mut values: BTreeMap<String, SignInValue> = BTreeMap::new();
    for (name, text) in &given {
        if secret_fields.contains(name) {
            put_provider_secret(
                &mut tx,
                id,
                &format!("{LOGIN_SECRET_PREFIX}{name}"),
                text,
                by,
                ctx.now,
            )
            .await?;
        } else {
            values.insert(name.clone(), SignInValue::Text(text.clone()));
        }
    }
    for (name, secret) in rows::login_secrets(&mut tx, id).await? {
        if let Some(field) = name.strip_prefix(LOGIN_SECRET_PREFIX) {
            values.insert(field.to_owned(), SignInValue::Secret(secret));
        }
    }
    let pending = Pending::new(&body.method, &values, by)
        .to_bytes()
        .map_err(ApiError::internal)?;
    rows::keep_pending(&mut tx, id, &pending, ctx.now).await?;
    record(
        ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::ProviderLogin),
        id,
        Home {
            node: Some(state.home_id),
            org: Some(state.org_id),
        },
        &format!(
            "{} began to sign the provider {} in by the method {}",
            crate::routes::nodes::actor_name(ctx)?,
            state.name,
            body.method
        ),
        json!({ "provider": id, "method": body.method, "fields": given.keys().collect::<Vec<_>>() }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    let before = current(ctx, id).await?;
    let delivered = ctx
        .app
        .engine
        .deliver_sign_in(id)
        .await
        .map_err(ApiError::internal)?;
    // A harness that had not said what it declares when the request came has said it by now: the sign-in
    // that imports a credential is done in this request, and its answer carries the end of it.
    let imports = match &method {
        Some(method) => method.kind == LoginKind::Import,
        None => {
            let state = signing_in_provider(ctx, id).await?;
            methods_of(&mut *ctx.app.db.acquire().await?, &state)
                .await?
                .and_then(|methods| methods.into_iter().find(|each| each.id == body.method))
                .is_some_and(|each| each.kind == LoginKind::Import)
        }
    };
    match delivered {
        Delivered::Sent if imports => {
            let end = settled(ctx, id, IMPORT_WAIT, &before).await?;
            if end.state == LoginProgress::Completed {
                // What the harness holds now is read at once, so that the answer carries a provider
                // that is signed in, or says why it is not.
                ctx.app
                    .engine
                    .look_at_completed_sign_in(id)
                    .await
                    .map_err(ApiError::internal)?;
            }
            Ok(end)
        }
        Delivered::Sent => settled(ctx, id, ANSWER_WAIT, &before).await,
        Delivered::Waits(_) | Delivered::Nothing | Delivered::Refused(_) => current(ctx, id).await,
    }
}

/// Signs a provider in, or in again, by one of its adapter's methods: the new credential replaces the
/// old one in place, and the provider's threads and their sessions are left as they are.
async fn start(ctx: Ctx, (): (), body: ProviderLoginStartBody) -> ApiResult<LoginState> {
    let id = ctx.id("id")?;
    begin(&ctx, id, &body).await
}

// ---------------------------------------------------------------- the rest of it

/// Gives back what a running sign-in asked for: the values are checked against the fields it asks for
/// now, the secrets among them sealed at once, and the harness is told.
async fn input(ctx: Ctx, (): (), body: ProviderLoginInputBody) -> ApiResult<LoginState> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, _) = begin_mutation(&ctx).await?;
    let by = ctx.user()?.id();
    let state = rows::state(&mut tx, id)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    subscription(&state)?;
    admit(&mut tx, id).await?;
    let before = shown(state.login_state.as_ref());
    let Some(asks) = before
        .asks
        .clone()
        .filter(|_| before.state == LoginProgress::AwaitingUser)
    else {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "no sign-in waits for an answer",
        ));
    };
    checked(&asks, &body.values, "values.")?;
    rows::delete_login_secrets(&mut tx, id, LOGIN_SECRET_PREFIX).await?;
    let mut values: BTreeMap<String, SignInValue> = BTreeMap::new();
    for (name, text) in &body.values {
        let secret = asks
            .iter()
            .find(|field| &field.name == name)
            .is_some_and(|field| field.kind.is_secret());
        if secret {
            put_provider_secret(
                &mut tx,
                id,
                &format!("{LOGIN_SECRET_PREFIX}{name}"),
                text,
                by,
                ctx.now,
            )
            .await?;
        } else {
            values.insert(name.clone(), SignInValue::Text(text.clone()));
        }
    }
    for (name, secret) in rows::login_secrets(&mut tx, id).await? {
        if let Some(field) = name.strip_prefix(LOGIN_SECRET_PREFIX) {
            values.insert(field.to_owned(), SignInValue::Secret(secret));
        }
    }
    record(
        &ctx,
        &mut tx,
        AuditAction::ProviderLogin,
        id,
        Home {
            node: Some(state.home_id),
            org: Some(state.org_id),
        },
        "The caller answered the provider's sign-in",
        json!({"provider": id, "fields": body.values.keys().collect::<Vec<_>>()}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    let asked = ctx
        .app
        .engine
        .send_login_input(id, &values)
        .await
        .map_err(ApiError::internal)?;
    refused_or(asked)?;
    settled(&ctx, id, ANSWER_WAIT, &before).await
}

/// Reads where a provider's sign-in stands.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<LoginState> {
    let id = ctx.id("id")?;
    signing_in_provider(&ctx, id).await?;
    current(&ctx, id).await
}

/// Cancels a running sign-in, and forgets one that waits for the runtime, with its secrets.
async fn cancel(ctx: Ctx, (): (), (): ()) -> ApiResult<LoginState> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let state = rows::state(&mut tx, id)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    subscription(&state)?;
    let asked = ctx
        .app
        .engine
        .cancel_sign_in_in(tx, &principal, &crate::sign_in::origin(&ctx), id)
        .await
        .map_err(ApiError::internal)?;
    // A runtime that is not up has nothing running to tell: what waited is gone all the same.
    if !matches!(asked, Asked::NotUp) {
        refused_or(asked)?;
    }
    current(&ctx, id).await
}

/// Signs a provider's harness out: it forgets the login it holds, and the provider needs a sign-in.
async fn logout(ctx: Ctx, (): (), (): ()) -> ApiResult<Provider> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let state = rows::state(&mut tx, id)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    subscription(&state)?;
    admit(&mut tx, id).await?;
    let asked = ctx
        .app
        .engine
        .sign_out_in(tx, &principal, &crate::sign_in::origin(&ctx), id)
        .await
        .map_err(ApiError::internal)?;
    if asked == Asked::NotUp {
        // Its harness cannot be asked while the unit is not up: the unit is started, and the person
        // signs out when it is.
        let _ = ctx
            .app
            .engine
            .start_provider_runtime(id, Some(ctx.user()?.id()))
            .await;
    }
    refused_or(asked)?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::ProviderLogout),
        id,
        Home {
            node: Some(state.home_id),
            org: Some(state.org_id),
        },
        &format!(
            "{} signed the provider {} out",
            crate::routes::nodes::actor_name(&ctx)?,
            state.name
        ),
        json!({ "provider": id }),
    )
    .await?;
    let provider = show(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(provider)
}

/// What a harness that did not do what it was asked is to the caller: its words, whole.
fn refused_or(asked: Asked) -> ApiResult<()> {
    match asked {
        Asked::Done => Ok(()),
        Asked::NotUp => Err(ApiError::new(
            ErrorCode::ProviderUnavailable,
            "the provider's runtime is not running: it is started, and this is done when it is up",
        )),
        Asked::Refused(words) => Err(ApiError::new(
            ErrorCode::ProviderUnavailable,
            format!("the harness did not do it: {words}"),
        )),
    }
}

/// Which values, among those a method asks for, are secrets, by name.
#[must_use]
pub fn secret_fields_of(method: &LoginMethodDef) -> Vec<String> {
    method
        .fields
        .iter()
        .filter(|field| field.kind.is_secret())
        .map(|field| field.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::Choice;

    fn fields() -> Vec<Field> {
        let mut tier = Field::new("tier", "Tier", FieldType::Choice);
        tier.choices = Some(vec![Choice {
            value: "pro".to_owned(),
            label: "Pro".to_owned(),
        }]);
        tier.required = false;
        let optional = |name: &str, kind| {
            let mut field = Field::new(name, name, kind);
            field.required = false;
            field
        };
        vec![
            optional("account", FieldType::Text),
            optional("token", FieldType::Secret),
            tier,
            optional("verified", FieldType::Boolean),
            optional("expires_at", FieldType::Date),
            optional("labels", FieldType::Map),
            optional("headers", FieldType::SecretMap),
        ]
    }

    fn given(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn what_a_person_typed_is_read_as_the_type_of_the_field_it_was_typed_for() {
        let values = checked(
            &fields(),
            &given(&[
                ("account", "ada"),
                ("verified", "true"),
                ("labels", r#"{"team":"core"}"#),
                ("headers", r#"{"x-key":"k"}"#),
                ("tier", "pro"),
                ("expires_at", "2027-01-31"),
            ]),
            "values.",
        )
        .unwrap();
        assert_eq!(values["verified"], Json::Bool(true));
        assert_eq!(values["labels"], json!({ "team": "core" }));
        assert_eq!(values["account"], json!("ada"));
    }

    #[test]
    fn each_value_that_does_not_fit_its_field_is_named_by_the_field() {
        let error = checked(
            &fields(),
            &given(&[
                ("verified", "perhaps"),
                ("labels", "not an object"),
                ("tier", "gold"),
                ("expires_at", "next week"),
                ("nonsense", "1"),
            ]),
            "values.",
        )
        .unwrap_err();
        let errors = error.problem.extra["errors"].as_array().unwrap();
        let paths: Vec<&str> = errors
            .iter()
            .map(|each| each["path"].as_str().unwrap())
            .collect();
        for expected in ["values.verified", "values.labels", "values.nonsense"] {
            assert!(paths.contains(&expected), "{paths:?}");
        }
    }

    #[test]
    fn a_text_longer_than_a_field_takes_is_refused_and_a_secret_field_is_known_by_its_type() {
        let long = "x".repeat(FIELD_BYTES_DEFAULT + 1);
        assert!(checked(&fields(), &given(&[("token", &long)]), "values.").is_err());
        let method = LoginMethodDef {
            id: "import".to_owned(),
            title: String::new(),
            summary: String::new(),
            kind: LoginKind::Import,
            fields: fields(),
            yields: orch_agent_contract::login::LoginYields::Token,
            lifetime_secs: None,
            requires: None,
            recommended: false,
        };
        assert_eq!(secret_fields_of(&method), ["token", "headers"]);
    }

    #[test]
    fn a_sign_in_nobody_began_is_shown_as_none_running() {
        let idle = shown(None);
        assert_eq!(idle.state, LoginProgress::Idle);
        assert!(idle.shows.is_empty() && idle.asks.is_none() && idle.error.is_none());
        let told = json!({
            "state": "awaiting_user", "method": "device_code",
            "shows": [{ "kind": "code", "label": "Code", "value": "ABCD" }],
            "asks": null, "expires_at": null, "error": null,
            "at": "2026-10-12T00:00:00Z"
        });
        let shown = shown(Some(&told));
        assert_eq!(shown.state, LoginProgress::AwaitingUser);
        assert_eq!(shown.shows[0].value, "ABCD");
    }
}
