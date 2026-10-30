//! The single sign-in methods a system admin keeps: listed, added, changed by revision, tested against the provider itself, and removed unless somebody could then no longer sign in.
//!
//! The client secret is the one thing here that is never read back: it is sealed under the system's
//! key as it arrives, bound to its own row, and a provider says only whether one is set. The address
//! the provider is told to call back is made from the system's public address and the provider's key.
//! **Test** asks the kind that speaks the provider's protocol to read the issuer's description of
//! itself and its keys, and answers what was found, or the words of what went wrong; it signs nobody
//! in and changes nothing.

use super::{admin_of, invalid, record};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::auth::oidc::{kind_of, settings_of};
use crate::routes::nodes::placed;
use crate::routes::resources::sealing::{binding, master_key};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::access::Role;
use orch_api_contract::admin::{
    AdminIdentityProviderCreate, AdminIdentityProviderDelete, AdminIdentityProviderList,
    AdminIdentityProviderTest, AdminIdentityProviderUpdate, IdentityProvider,
    IdentityProviderInput, IdentityProviderTestReply, IdentityProviderUpdateBody,
};
use orch_api_contract::common::{BASE_PATH, ErrorCode};
use orch_config::ServerConfig;
use orch_db::queries::hierarchy;
use orch_db::queries::identity::{
    self, IDENTITY_PROVIDER_SECRET_NAME, IDENTITY_PROVIDER_SECRET_OWNER, IdentityProviderChange,
    IdentityProviderRow, NewIdentityProvider,
};
use orch_db::queries::settings::put_secret;
use orch_db::{Conn, DbError};
use orch_identity_contract::provider::{KIND_OIDC, issuer_fault};
use orch_types::Id;
use serde_json::json;

/// The fewest and the most characters of a key: what the schema allows.
const KEY_CHARS: std::ops::RangeInclusive<usize> = 2..=32;
/// The most characters of the label of a button.
const NAME_MAX_CHARS: usize = 80;
/// The most characters of a client identifier, a claim or a scope.
const WORD_MAX_CHARS: usize = 256;
/// The scopes asked for when none are given.
const DEFAULT_SCOPES: [&str; 3] = ["openid", "profile", "email"];
/// The scope without which there is no sign-in through OpenID Connect.
const OPENID_SCOPE: &str = "openid";
/// The claims read when none are named: the subject, the address, the name and the username.
const DEFAULT_CLAIMS: [&str; 4] = ["sub", "email", "name", "preferred_username"];
/// How many people a refusal to remove a provider names; the rest are counted.
const NAMED_PEOPLE: usize = 20;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminIdentityProviderList, list)
        .on(AdminIdentityProviderCreate, create)
        .on(AdminIdentityProviderUpdate, update)
        .on(AdminIdentityProviderDelete, delete)
        .on(AdminIdentityProviderTest, test);
}

/// The address a provider is told to call back.
fn callback_url(config: &ServerConfig, key: &str) -> String {
    format!(
        "{}{BASE_PATH}/auth/oidc/{key}/callback",
        config.system.public_url.trim_end_matches('/')
    )
}

fn shown(config: &ServerConfig, row: IdentityProviderRow) -> IdentityProvider {
    IdentityProvider {
        callback_url: callback_url(config, &row.key),
        default_role: row
            .default_role
            .and_then(|word| serde_json::from_value::<Role>(json!(word)).ok()),
        id: row.id,
        key: row.key,
        name: row.name,
        issuer: row.issuer,
        client_id: row.client_id,
        scopes: row.scopes,
        claim_subject: row.claim_subject,
        claim_email: row.claim_email,
        claim_name: row.claim_name,
        claim_username: row.claim_username,
        allowed_email_domains: row.allowed_email_domains,
        auto_register: row.auto_register,
        default_org_id: row.default_org_id,
        enabled: row.enabled,
        client_secret_set: row.client_secret_set,
        revision: row.revision,
    }
}

/// Every identity provider.
async fn list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<IdentityProvider>> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    Ok(identity::identity_providers(&mut conn)
        .await?
        .into_iter()
        .map(|row| shown(&ctx.app.config, row))
        .collect())
}

fn key_fits(key: &str) -> bool {
    KEY_CHARS.contains(&key.len())
        && key.starts_with(|first: char| first.is_ascii_lowercase())
        && key
            .chars()
            .all(|each| each.is_ascii_lowercase() || each.is_ascii_digit() || each == '-')
}

fn clean_key(key: &str) -> ApiResult<String> {
    let key = key.trim();
    if key_fits(key) {
        Ok(key.to_owned())
    } else {
        Err(invalid(
            "key",
            "is 2 to 32 characters: lower-case letters, digits and -, beginning with a letter",
        ))
    }
}

fn clean_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
        Err(invalid("name", "must be 1 to 80 characters"))
    } else {
        Ok(name.to_owned())
    }
}

/// The issuer of a provider: an `https` address (or `http` for a provider on this very machine, in
/// development) without a user name, a query or a fragment, which is what its discovery document and
/// the tokens it signs name it by.
fn clean_issuer(issuer: &str) -> ApiResult<String> {
    let issuer = issuer.trim();
    match issuer_fault(issuer) {
        None => Ok(issuer.to_owned()),
        Some(fault) => Err(invalid("issuer", &fault)),
    }
}

/// A word that names a client, a scope or a claim: not empty, without space, not long.
fn clean_word(path: &str, word: &str) -> ApiResult<String> {
    let word = word.trim();
    if word.is_empty()
        || word.chars().count() > WORD_MAX_CHARS
        || word
            .chars()
            .any(|each| each.is_whitespace() || each.is_control())
    {
        Err(invalid(path, "must be 1 to 256 characters without a space"))
    } else {
        Ok(word.to_owned())
    }
}

/// The scopes asked for, each once; `openid` has to be among them.
fn clean_scopes(scopes: &[String]) -> ApiResult<Vec<String>> {
    let mut clean: Vec<String> = Vec::with_capacity(scopes.len());
    for (index, scope) in scopes.iter().enumerate() {
        let scope = clean_word(&format!("scopes[{index}]"), scope)?;
        if !clean.contains(&scope) {
            clean.push(scope);
        }
    }
    if clean.iter().any(|scope| scope == OPENID_SCOPE) {
        Ok(clean)
    } else {
        Err(invalid("scopes", "must include openid"))
    }
}

/// The domains of the addresses let in, in lower case, each once, without an `@`.
fn clean_domains(domains: &[String]) -> ApiResult<Vec<String>> {
    let mut clean: Vec<String> = Vec::with_capacity(domains.len());
    for (index, domain) in domains.iter().enumerate() {
        let domain = domain.trim().to_lowercase();
        if domain.is_empty()
            || domain.contains(['@', '/'])
            || domain.chars().any(char::is_whitespace)
            || !domain.contains('.')
        {
            return Err(invalid(
                &format!("allowed_email_domains[{index}]"),
                "is a domain such as example.com",
            ));
        }
        if !clean.contains(&domain) {
            clean.push(domain);
        }
    }
    Ok(clean)
}

fn role_word(role: Role) -> ApiResult<String> {
    serde_json::to_value(role)
        .ok()
        .and_then(|word| word.as_str().map(str::to_owned))
        .ok_or_else(|| ApiError::internal("a role is not a word"))
}

/// Checks that the org joined at auto-registration is there and takes members.
async fn check_default_org(conn: &mut Conn, org: Id) -> ApiResult<()> {
    let (found, kind) = placed(conn, org)
        .await
        .map_err(|_| invalid("default_org_id", "is no org of this system"))?;
    if kind != orch_api_contract::nodes::NodeKind::Org {
        return Err(invalid("default_org_id", "is no org"));
    }
    let root = hierarchy::org(conn, found.org)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if root.closed {
        return Err(ApiError::new(
            ErrorCode::OrgClosed,
            "that org takes no further member: people cannot be let into it when they sign in",
        ));
    }
    Ok(())
}

/// Seals the client secret as it arrives and keeps it, bound to its own row.
async fn keep_secret(
    ctx: &Ctx,
    conn: &mut Conn,
    provider: Id,
    admin: Id,
    secret: &str,
) -> ApiResult<()> {
    if secret.is_empty() {
        return Err(invalid("client_secret", "must not be empty"));
    }
    if secret.len() > orch_settings::secrets::SECRET_MAX_BYTES {
        return Err(
            ApiError::new(ErrorCode::RequestTooLarge, "the client secret is too large")
                .with("limit", orch_settings::secrets::SECRET_MAX_BYTES),
        );
    }
    let sealed = master_key()?
        .seal(
            secret.as_bytes(),
            &binding(
                IDENTITY_PROVIDER_SECRET_OWNER,
                provider,
                IDENTITY_PROVIDER_SECRET_NAME,
            ),
        )
        .map_err(|_| ApiError::internal("a secret could not be sealed"))?;
    put_secret(
        conn,
        IDENTITY_PROVIDER_SECRET_OWNER,
        provider,
        IDENTITY_PROVIDER_SECRET_NAME,
        &sealed,
        Some(admin),
        ctx.now,
    )
    .await?;
    Ok(())
}

/// `409 conflict.exists` for a key another provider has.
fn key_taken(error: DbError) -> ApiError {
    match error {
        DbError::Unique { .. } => {
            ApiError::new(ErrorCode::ConflictExists, "another provider has that key")
        }
        other => other.into(),
    }
}

/// Adds an identity provider.
async fn create(mut ctx: Ctx, (): (), body: IdentityProviderInput) -> ApiResult<IdentityProvider> {
    admin_of(&ctx)?;
    let key = clean_key(&body.key)?;
    let name = clean_name(&body.name)?;
    let issuer = clean_issuer(&body.issuer)?;
    let client_id = clean_word("client_id", &body.client_id)?;
    let scopes = match &body.scopes {
        Some(scopes) => clean_scopes(scopes)?,
        None => DEFAULT_SCOPES.map(str::to_owned).to_vec(),
    };
    let claim = |given: &Option<String>, path: &str, default: &str| -> ApiResult<String> {
        given
            .as_deref()
            .map_or_else(|| Ok(default.to_owned()), |claim| clean_word(path, claim))
    };
    let [subject, email, display, username] = DEFAULT_CLAIMS;
    let claims = [
        claim(&body.claim_subject, "claim_subject", subject)?,
        claim(&body.claim_email, "claim_email", email)?,
        claim(&body.claim_name, "claim_name", display)?,
        claim(&body.claim_username, "claim_username", username)?,
    ];
    let domains = clean_domains(body.allowed_email_domains.as_deref().unwrap_or_default())?;
    let role = body.default_role.map(role_word).transpose()?;
    if role.is_some() && body.default_org_id.is_none() {
        return Err(invalid(
            "default_role",
            "needs default_org_id: it is the role in that org",
        ));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    if let Some(org) = body.default_org_id {
        check_default_org(&mut tx, org).await?;
    }
    let id = orch_types::new_id();
    identity::insert_identity_provider(
        &mut tx,
        &NewIdentityProvider {
            id,
            key: &key,
            name: &name,
            issuer: &issuer,
            client_id: &client_id,
            scopes: &scopes,
            claim_subject: &claims[0],
            claim_email: &claims[1],
            claim_name: &claims[2],
            claim_username: &claims[3],
            allowed_email_domains: &domains,
            auto_register: body.auto_register.unwrap_or(false),
            default_org_id: body.default_org_id,
            default_role: role.as_deref(),
            enabled: body.enabled.unwrap_or(true),
        },
        ctx.now,
    )
    .await
    .map_err(key_taken)?;
    if let Some(secret) = &body.client_secret {
        keep_secret(&ctx, &mut tx, id, admin, secret).await?;
    }
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        orch_db::audit::action!(AuditAction::IdentityProviderCreated),
        Some(id),
        &format!("{who} added the identity provider {name}"),
        json!({ "key": key, "issuer": issuer, "secret_set": body.client_secret.is_some() }),
    )
    .await?;
    let row = identity::identity_provider(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::internal("a provider was added and is not there"))?;
    orch_db::sequences::notify_meta_changed(&mut tx, Some(ctx.request_id.as_str())).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown(&ctx.app.config, row))
}

/// Changes an identity provider that is at the revision the caller saw.
async fn update(
    mut ctx: Ctx,
    (): (),
    body: IdentityProviderUpdateBody,
) -> ApiResult<IdentityProvider> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let key = body.key.as_deref().map(clean_key).transpose()?;
    let name = body.name.as_deref().map(clean_name).transpose()?;
    let issuer = body.issuer.as_deref().map(clean_issuer).transpose()?;
    let client_id = body
        .client_id
        .as_deref()
        .map(|each| clean_word("client_id", each))
        .transpose()?;
    let scopes = body.scopes.as_deref().map(clean_scopes).transpose()?;
    let claim = |given: &Option<String>, path: &str| -> ApiResult<Option<String>> {
        given
            .as_deref()
            .map(|each| clean_word(path, each))
            .transpose()
    };
    let claim_subject = claim(&body.claim_subject, "claim_subject")?;
    let claim_email = claim(&body.claim_email, "claim_email")?;
    let claim_name = claim(&body.claim_name, "claim_name")?;
    let claim_username = claim(&body.claim_username, "claim_username")?;
    let domains = body
        .allowed_email_domains
        .as_deref()
        .map(clean_domains)
        .transpose()?;
    let role: Option<Option<String>> = body
        .default_role
        .map(|role| role.map(role_word).transpose())
        .transpose()?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let before = identity::identity_provider(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if before.revision != body.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the provider was changed since it was read",
        )
        .with("current", shown(&ctx.app.config, before)));
    }
    let org_after = body.default_org_id.unwrap_or(before.default_org_id);
    let role_after = role.clone().unwrap_or_else(|| before.default_role.clone());
    if role_after.is_some() && org_after.is_none() {
        return Err(invalid(
            "default_role",
            "needs default_org_id: it is the role in that org",
        ));
    }
    if let Some(Some(org)) = body.default_org_id {
        check_default_org(&mut tx, org).await?;
    }
    let mut changed: Vec<&str> = Vec::new();
    for (member, present) in [
        ("key", key.is_some()),
        ("name", name.is_some()),
        ("issuer", issuer.is_some()),
        ("client_id", client_id.is_some()),
        ("client_secret", body.client_secret.is_some()),
        ("scopes", scopes.is_some()),
        ("claim_subject", claim_subject.is_some()),
        ("claim_email", claim_email.is_some()),
        ("claim_name", claim_name.is_some()),
        ("claim_username", claim_username.is_some()),
        ("allowed_email_domains", domains.is_some()),
        ("auto_register", body.auto_register.is_some()),
        ("default_org_id", body.default_org_id.is_some()),
        ("default_role", role.is_some()),
        ("enabled", body.enabled.is_some()),
    ] {
        if present {
            changed.push(member);
        }
    }
    if changed.is_empty() {
        return Ok(shown(&ctx.app.config, before));
    }
    let after = identity::change_identity_provider(
        &mut tx,
        id,
        body.expected_revision,
        &IdentityProviderChange {
            key: key.as_deref(),
            name: name.as_deref(),
            issuer: issuer.as_deref(),
            client_id: client_id.as_deref(),
            scopes: scopes.as_deref(),
            claim_subject: claim_subject.as_deref(),
            claim_email: claim_email.as_deref(),
            claim_name: claim_name.as_deref(),
            claim_username: claim_username.as_deref(),
            allowed_email_domains: domains.as_deref(),
            auto_register: body.auto_register,
            default_org_id: body.default_org_id,
            default_role: role.as_ref().map(Option::as_deref),
            enabled: body.enabled,
        },
        ctx.now,
    )
    .await
    .map_err(key_taken)?
    .ok_or_else(ApiError::not_found)?;
    if let Some(secret) = &body.client_secret {
        keep_secret(&ctx, &mut tx, id, admin, secret).await?;
    }
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        orch_db::audit::action!(AuditAction::IdentityProviderUpdated),
        Some(id),
        &format!("{who} changed the identity provider {}", after.name),
        json!({ "key": after.key, "changed": changed }),
    )
    .await?;
    let row = identity::identity_provider(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::internal("a provider was changed and is not there"))?;
    orch_db::sequences::notify_meta_changed(&mut tx, Some(ctx.request_id.as_str())).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown(&ctx.app.config, row))
}

/// Removes an identity provider, unless somebody who can sign in only through it would be shut out.
async fn delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let row = identity::identity_provider(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let shut_out = identity::only_signed_in_through(&mut tx, id).await?;
    if !shut_out.is_empty() {
        let more = shut_out.len().saturating_sub(NAMED_PEOPLE);
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "this is the only way some people sign in: they would be shut out",
        )
        .with(
            "users",
            shut_out.into_iter().take(NAMED_PEOPLE).collect::<Vec<_>>(),
        )
        .with("more", more));
    }
    identity::delete_identity_provider(&mut tx, id).await?;
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        orch_db::audit::action!(AuditAction::IdentityProviderDeleted),
        Some(id),
        &format!("{who} removed the identity provider {}", row.name),
        json!({ "key": row.key }),
    )
    .await?;
    orch_db::sequences::notify_meta_changed(&mut tx, Some(ctx.request_id.as_str())).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Looks at a provider itself: reads its description and its keys through the kind that speaks its
/// protocol, and answers what was found, or the words of what went wrong. The admin is checked under
/// the guard while the provider is read; the provider is asked after that transaction, which no
/// request to another server holds open.
async fn test(ctx: Ctx, (): (), (): ()) -> ApiResult<IdentityProviderTestReply> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let row = {
        let (mut tx, _guard) = transaction_guard::begin(&ctx).await?;
        let row = identity::identity_provider(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        tx.commit().await.map_err(DbError::from)?;
        row
    };
    let failed = |error: String| IdentityProviderTestReply {
        ok: false,
        error: Some(error),
        issuer_metadata: None,
    };
    let Some(kind) = kind_of(KIND_OIDC) else {
        return Ok(failed(
            "No kind of identity provider is installed in this server".to_owned(),
        ));
    };
    let settings = settings_of(&row);
    if let Err(faults) = kind.check(&settings) {
        let said: Vec<String> = faults
            .into_iter()
            .map(|fault| format!("{} {}", fault.path, fault.message))
            .collect();
        return Ok(failed(format!(
            "The provider's settings are not right: {}",
            said.join("; ")
        )));
    }
    Ok(match kind.validate(&settings).await {
        Ok(found) => IdentityProviderTestReply {
            ok: true,
            error: None,
            issuer_metadata: Some(found.issuer_metadata()),
        },
        Err(error) => failed(error.detail),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_what_appears_in_a_callback_address() {
        for good in ["corp", "ab", "my-sso-1"] {
            assert!(key_fits(good), "{good}");
        }
        for bad in ["a", "1corp", "Corp", "co_rp", "co rp", &"k".repeat(33)] {
            assert!(!key_fits(bad), "{bad}");
        }
    }

    #[test]
    fn scopes_must_include_openid_and_are_kept_once() {
        let scopes =
            |list: &[&str]| -> Vec<String> { list.iter().map(|s| (*s).to_owned()).collect() };
        assert_eq!(
            clean_scopes(&scopes(&["openid", "email", "openid"])).unwrap(),
            scopes(&["openid", "email"])
        );
        assert!(clean_scopes(&scopes(&["email"])).is_err());
        assert!(clean_scopes(&scopes(&["open id"])).is_err());
    }

    #[test]
    fn domains_are_lower_case_dotted_names_without_an_at_sign() {
        let domains =
            |list: &[&str]| -> Vec<String> { list.iter().map(|s| (*s).to_owned()).collect() };
        assert_eq!(
            clean_domains(&domains(&["Example.COM", "example.com"])).unwrap(),
            domains(&["example.com"])
        );
        assert!(clean_domains(&domains(&["a@example.com"])).is_err());
        assert!(clean_domains(&domains(&["localhost"])).is_err());
    }
}
