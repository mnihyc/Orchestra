//! API tokens made narrow and with a fresh proof, within the longest life their places admit, asked again by their key without their secret; and an org's list of the tokens that reach it, each of which it can refuse there.
//!
//! A token made without scopes may use nothing: the page's presets send an
//! explicit list, and everything its person may only when `*` is chosen. The
//! request that makes it carries the maker's fresh proof ([`fresh`]). Its whole
//! life, from now to its end, is checked against `tokens.max_days` where it is
//! made: the system's for a personal token, the orgs down to its node or to the
//! service identity's org as well; an org it reaches later is checked when it
//! is used (`authz` keeps it out). A creation asked again with the same
//! `Idempotency-Key` answers what was made, with no secret: none is kept to be
//! given twice. An org's members managers list the tokens that can reach it and
//! refuse one there, which keeps it out of the org and everything inside from
//! the next request on, and changes nothing elsewhere.
//!
//! [`fresh`]: crate::sign_in::fresh

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::sign_in::record;

use chrono::DateTime;

use chrono::Utc;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::decisions::Scopes;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::auth::ApiTokenCreateBody;

use orch_api_contract::auth::ApiTokenInfo;

use orch_api_contract::auth::OrgToken;

use orch_api_contract::auth::OrgTokenList;

use orch_api_contract::auth::OrgTokenListQuery;

use orch_api_contract::auth::OrgTokenRefuse;

use orch_api_contract::auth::TokenCreated;

use orch_api_contract::common::DEFAULT_LIMIT;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::FieldFault;

use orch_api_contract::common::IDEMPOTENCY_HEADER;

use orch_api_contract::common::MAX_LIMIT;

use orch_api_contract::common::Page;

use orch_api_contract::common::Problem;

use orch_db::Conn;

use orch_db::audit::Actor;

use orch_db::idempotency;

use orch_db::idempotency::Replay;

use orch_db::queries::tokens;

use orch_settings::Place;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde_json::json;
/// How many characters a token's name has.
const NAME_CHARS: std::ops::RangeInclusive<usize> = 1..=80;

/// Mounts the org's list of tokens and the refusal.
pub fn mount(registry: &mut Registry) {
    registry
        .on(
            orch_api_contract::operations::OperationCatalogueGet,
            operation_catalogue,
        )
        .on_with_operation(OrgTokenList, org_inventory::list, org_inventory::get)
        .on_with_operation(OrgTokenRefuse, org_inventory::refusal, refuse);
}

async fn operation_catalogue(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<Vec<orch_api_contract::operations::Operation>> {
    let _ = ctx.user()?;
    orch_api_contract::operations::catalogue().map_err(ApiError::internal)
}

/// What a request to make a token asks for, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// Its name.
    pub name: String,
    /// Its scope words: none where none were given.
    pub scopes: Vec<String>,
    /// Explicit operation mode, including server-retained issuer ceilings.
    pub operation_scope: Option<KeyScope>,
    /// When it ends.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Checks the body of a request to make a token, all of it at once: a name, scopes of the catalogue
/// (left out, none), an end in the future.
pub fn asked(body: &ApiTokenCreateBody, now: DateTime<Utc>) -> ApiResult<Asked> {
    let mut faults = Vec::new();
    let mut fault = |path: &str, message: &str| {
        faults.push(FieldFault {
            path: path.to_owned(),
            message: message.to_owned(),
        });
    };
    let name = body.name.trim().to_owned();
    if !NAME_CHARS.contains(&name.chars().count()) {
        fault("name", "must be 1 to 80 characters");
    }
    if body.rules.is_some() && (body.scopes.is_some() || body.node_id.is_some()) {
        fault("rules", "cannot be combined with legacy scopes or node_id");
    }
    let operation_scope = if let Some(rules) = &body.rules {
        Some(KeyScope {
            rules: rules.clone(),
            ..KeyScope::default()
        })
    } else if body.scopes.is_none() && body.node_id.is_none() {
        Some(KeyScope::default())
    } else {
        None
    };
    if let Some(scope) = &operation_scope {
        for error in scope.validate() {
            fault(&error.path, &error.message);
        }
        for error in
            orch_api_contract::operations::validate_selections(scope).map_err(ApiError::internal)?
        {
            fault(&error.path, &error.message);
        }
    }
    let scopes = match Scopes::parse(body.scopes.as_deref().unwrap_or_default()) {
        Ok(parsed) => parsed.to_strings(),
        Err(error) => {
            fault("scopes", &error.to_string());
            Vec::new()
        }
    };
    let expires_at = body.expires_at.map(Time::get);
    if expires_at.is_some_and(|at| at <= now) {
        fault("expires_at", "must be in the future");
    }
    if faults.is_empty() {
        Ok(Asked {
            name,
            scopes,
            operation_scope,
            expires_at,
        })
    } else {
        Err(Problem::invalid(faults).into())
    }
}

impl Asked {
    /// Applies the current guarded caller's complete policy ceiling before the same token writer
    /// runs. The required key-management facet must already have passed the shared operation gate.
    pub fn for_issuer(
        mut self,
        caller: &orch_authz::Caller,
        route: &orch_api_contract::routes::Route,
        account: Id,
    ) -> ApiResult<Self> {
        if caller.grant().is_none() {
            return Ok(self);
        }
        let account = KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            id: Some(account),
        };
        let operation =
            orch_api_contract::operations::for_route(route).map_err(ApiError::internal)?;
        if !caller.admitted_facets().is_some_and(|facets| {
            operation.variants.iter().any(|variant| {
                let required: Vec<_> = variant
                    .facets
                    .iter()
                    .filter(|facet| {
                        facet.usage == orch_api_contract::operations::FacetUsage::Required
                    })
                    .collect();
                !required.is_empty()
                    && required.iter().all(|required| {
                        facets.iter().any(|facet| {
                            facet.operation == variant.operation
                                && facet.action == required.action
                                && required.resources.contains(&facet.resource_kind)
                                && facet.target.object == account
                        })
                    })
            })
        }) {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "this key issuance has not passed its current operation scope",
            ));
        }
        let Some(scope) = self.operation_scope else {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "a key cannot issue a legacy credential outside its operation ceiling",
            ));
        };
        self.operation_scope = Some(
            caller
                .attenuate_key_scope(scope.rules, self.expires_at.map(Time::new))
                .map_err(|error| ApiError::new(ErrorCode::AuthzDenied, error.to_string()))?,
        );
        Ok(self)
    }

    /// Exact stored restriction envelope; absence is explicitly selected legacy mode.
    pub fn stored_scope(&self) -> ApiResult<Option<Json>> {
        self.operation_scope
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(ApiError::internal)
    }

    /// Newly written operation-mode metadata, without reading existing node names for a receipt.
    pub fn created_info(&self, id: Id, now: DateTime<Utc>) -> ApiResult<ApiTokenInfo> {
        let scope = self
            .operation_scope
            .as_ref()
            .ok_or_else(|| ApiError::internal("a scoped receipt needs operation-mode issuance"))?;
        Ok(ApiTokenInfo {
            id,
            name: self.name.clone(),
            scopes: self.scopes.clone(),
            rules: Some(scope.rules.clone()),
            constraints: scope.constraints.clone(),
            excluded_places: scope.excluded_places.clone(),
            node: None,
            created_at: Time::new(now),
            last_used_at: None,
            expires_at: self.expires_at.map(Time::new),
        })
    }
}

/// Truthful owner-visible policy metadata. A malformed stored policy is never described as legacy.
pub fn scope_info(raw: Option<Json>) -> ApiResult<Option<KeyScope>> {
    let scope: Option<KeyScope> = raw
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?;
    if scope
        .as_ref()
        .is_some_and(|scope| !scope.validate().is_empty())
    {
        return Err(ApiError::internal(
            "a stored key has an invalid operation scope",
        ));
    }
    Ok(scope)
}
orch_api_base::__api_bind_ff99c0b88b!();

/// Refuses a token whose end is no longer in the future at `now`, the action's own time (an end that
/// passed while the action waited for its turn is refused as it would have been at once), and one whose
/// whole life, from `now` to `expires_at` (none: it never ends), is longer than `tokens.max_days` at
/// `place`: `403 token.lifetime_exceeded` with the bound and the place it comes from.
pub async fn within_lifetime(
    conn: &mut Conn,
    place: Place,
    now: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
) -> ApiResult<()> {
    if expires_at.is_some_and(|end| end <= now) {
        return Err(Problem::invalid(vec![FieldFault {
            path: "expires_at".to_owned(),
            message: "must be in the future".to_owned(),
        }])
        .into());
    }
    let view = orch_settings::resolve(conn, place, keys::TOKENS_MAX_DAYS)
        .await
        .map_err(ApiError::internal)?;
    let Some(max_days) = view
        .resolved
        .value
        .as_ref()
        .and_then(serde_json::Value::as_i64)
    else {
        return Ok(());
    };
    let longest = chrono::Duration::days(max_days);
    if expires_at.is_some_and(|end| end - now <= longest) {
        return Ok(());
    }
    Err(ApiError::new(
        ErrorCode::TokenLifetimeExceeded,
        format!("a token made here may live {max_days} days at most"),
    )
    .with("max_days", max_days)
    .with("scope", view.resolved.source.node))
}

/// The key the request carries, where it carries one.
fn key_of(ctx: &Ctx) -> Option<String> {
    ctx.headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

fn issuance_path(ctx: &Ctx) -> ApiResult<String> {
    let mut path = ctx.route.path.to_owned();
    for name in ctx.route.parameters() {
        path = path.replace(&format!("{{{name}}}"), &ctx.id(name)?.to_string());
    }
    Ok(path)
}

/// What was answered to the same creation asked before with this request's key, without its secret:
/// looked at before any proof is spent. The same key on another request is a mistake of the client.
pub async fn replayed(ctx: &Ctx, conn: &mut Conn, maker: Id) -> ApiResult<Option<TokenCreated>> {
    let Some(key) = key_of(ctx) else {
        return Ok(None);
    };
    let since = crate::middleware::idempotency::window_since(conn, ctx.now).await?;
    let Some(replay) = idempotency::find(conn, maker, &key, since).await? else {
        return Ok(None);
    };
    let path = issuance_path(ctx)?;
    if replay.method != "POST" || (replay.path != path && replay.path != ctx.route.path) {
        return Err(ApiError::new(
            ErrorCode::ConflictIdempotencyMismatch,
            "the key was used for another request",
        ));
    }
    let info: ApiTokenInfo = serde_json::from_value(replay.response).map_err(ApiError::internal)?;
    if replay.path != path {
        // Old service-token receipts stored only the route template. Confirm the actual
        // account from the existing credential row instead of replaying across accounts.
        let account: Option<Id> = sqlx::query_scalar("SELECT user_id FROM api_tokens WHERE id=$1")
            .bind(info.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        if account != Some(ctx.id("id")?) {
            return Err(ApiError::new(
                ErrorCode::ConflictIdempotencyMismatch,
                "the key was used for another account",
            ));
        }
    }
    crate::key_operations::activity::replayed();
    Ok(Some(TokenCreated {
        token: None,
        info,
        already_created: true,
    }))
}

/// Keeps, under the request's key, what a repeat of this creation is answered: the token as it is
/// listed, never its secret. In the transaction that made it.
pub async fn keep(ctx: &Ctx, conn: &mut Conn, maker: Id, info: &ApiTokenInfo) -> ApiResult<()> {
    let Some(key) = key_of(ctx) else {
        return Ok(());
    };
    let replay = Replay {
        method: "POST".to_owned(),
        path: issuance_path(ctx)?,
        status: 200,
        response: serde_json::to_value(info).map_err(ApiError::internal)?,
    };
    idempotency::keep(conn, maker, &key, &replay, ctx.now).await?;
    Ok(())
}

/// Refuses a token in the org and everything inside, once: the token keeps working elsewhere, and
/// nothing it did before is undone. What was decided under its reach is decided again.
async fn refuse(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let org = ctx.id("id")?;
    let token = ctx.id("token_id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let reaching = tokens::reaching(&mut tx, org, ctx.now, None, i64::MAX).await?;
    if !reaching.iter().any(|row| row.key.id == token) {
        return Err(ApiError::not_found());
    }
    if tokens::refuse(&mut tx, token, org, principal.user(), ctx.now).await? {
        orch_db::queries::access::bump_epoch(&mut tx).await?;
        record(
            &ctx,
            &mut tx,
            Actor::User(principal.user()),
            orch_db::audit::action!(AuditAction::AuthTokenRefused),
            Some(token),
            "a token was refused in this org",
            json!({ "token": token, "org": org }),
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

#[path = "tokens/org_inventory.rs"]
pub mod org_inventory;

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used, reason = "a boundary control fails by panicking")]
pub mod tests {

    use super::*;

    use orch_access_contract::key_scope::KeyAction;

    use orch_access_contract::key_scope::KeyFacet;

    use orch_access_contract::key_scope::KeyResolvedTarget;

    use orch_api_contract::auth::MeTokenCreate;

    use orch_api_contract::routes::Endpoint;

    use orch_authz::Caller;

    use orch_authz::TokenGrant;

    use orch_types::new_id;
    fn body(value: Json) -> ApiTokenCreateBody {
        serde_json::from_value(value).unwrap()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn omitted_modes_create_explicit_deny_all_and_explicit_legacy_fields_stay_distinct() {
        let now = orch_testkit::clock::start();
        let blank = asked(&body(json!({"name":"empty"})), now).unwrap();
        assert_eq!(blank.operation_scope, Some(KeyScope::default()));
        assert!(blank.scopes.is_empty());
        let old = asked(&body(json!({"name":"legacy","scopes":["doc.view"]})), now).unwrap();
        assert!(old.operation_scope.is_none());
        assert_eq!(old.scopes, ["doc.view"]);
        assert!(
            asked(
                &body(json!({"name":"ambiguous","rules":[],"scopes":[]})),
                now
            )
            .is_err()
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_omitted_modes_create_explicit_deny_all_and_explicit_legacy_fields_stay_distinct()
     {
        self::omitted_modes_create_explicit_deny_all_and_explicit_legacy_fields_stay_distinct()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn issuance_rejects_unknown_future_selections_before_storage() {
        let now = orch_testkit::clock::start();
        let request = body(json!({"name":"future","rules":[{
            "target":{"kind":"account","id":new_id(),"reach":"self"},
            "allow":[{"operation":"UnknownFutureOperation","actions":["manage"]}]
        }]}));
        assert!(asked(&request, now).is_err());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_issuance_rejects_unknown_future_selections_before_storage() {
        self::issuance_rejects_unknown_future_selections_before_storage()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn key_issuance_requires_complete_current_admission_and_cannot_issue_a_legacy_escape() {
        let now = orch_testkit::clock::start();
        let user = new_id();
        let policy: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"account","id":user,"reach":"self"},
            "allow":[{"operation":"MeTokenCreate","actions":["manage","read"]}]
        }]}))
        .unwrap();
        let key = Caller::token(
            user,
            TokenGrant::from_stored_scope(&[], None, Some(policy.clone()), None).unwrap(),
        );
        let request = body(json!({"name":"child","rules":[]}));
        assert!(
            asked(&request, now)
                .unwrap()
                .for_issuer(&key, &MeTokenCreate::ROUTE, user)
                .is_err()
        );
        let facet = |action| KeyFacet {
            operation: "MeTokenCreate".into(),
            action,
            resource_kind: KeyTargetKind::Identity(KeyIdentityKind::Key),
            target: KeyResolvedTarget {
                object: KeyObjectRef {
                    kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
                    id: Some(user),
                },
                node_ancestors: Vec::new(),
                document_ancestors: Vec::new(),
                credential_account: None,
            },
        };
        let read_only = key
            .admit_facets(&[facet(KeyAction::Read)])
            .unwrap()
            .into_caller();
        assert!(
            asked(&request, now)
                .unwrap()
                .for_issuer(&read_only, &MeTokenCreate::ROUTE, user)
                .is_err(),
            "the operation name without its required effect is not a complete admission"
        );
        let admitted = key
            .admit_facets(&[facet(KeyAction::Manage)])
            .unwrap()
            .into_caller();
        let child = asked(&request, now)
            .unwrap()
            .for_issuer(&admitted, &MeTokenCreate::ROUTE, user)
            .unwrap();
        let scope = child.operation_scope.as_ref().unwrap();
        assert!(scope.rules.is_empty());
        assert_eq!(scope.constraints, vec![policy.rules]);
        let info = child.created_info(new_id(), now).unwrap();
        assert_eq!(info.rules, Some(Vec::new()));
        assert_eq!(info.constraints, scope.constraints);
        let legacy = asked(&body(json!({"name":"escape","scopes":["*"]})), now).unwrap();
        assert!(
            legacy
                .for_issuer(&admitted, &MeTokenCreate::ROUTE, user)
                .is_err()
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_key_issuance_requires_complete_current_admission_and_cannot_issue_a_legacy_escape()
     {
        self::key_issuance_requires_complete_current_admission_and_cannot_issue_a_legacy_escape()
    }
}
