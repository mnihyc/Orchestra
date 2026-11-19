//! Making a person: the rules a username, a name, an address and a password meet, and the user with the org that is theirs.
//!
//! Whoever is made, by registering or by the first setup, has an org of their
//! own from that moment, empty, closed and flat as the system's policies say
//! for a new one, and named as theirs in its Owners row, all in one
//! transaction.

use super::{passwords, record};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::pages::StoredKind;
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_config::server::Auth;
use orch_db::audit::Actor;
use orch_db::queries::identity::{self, UserRow};
use orch_db::{Conn, DbError};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::Time;
use serde_json::json;

/// The fewest and the most characters of a username: what the schema allows.
const USERNAME_CHARS: std::ops::RangeInclusive<usize> = 3..=32;

/// The fewest and the most characters of a display name: what the schema
/// allows.
const DISPLAY_NAME_CHARS: std::ops::RangeInclusive<usize> = 1..=64;

/// The most characters of an address: what mail allows of the whole of one.
const EMAIL_MAX_CHARS: usize = 254;

/// What a person gives to be made.
#[derive(Debug, Clone, Copy)]
pub struct Details<'a> {
    /// The username to sign in with.
    pub username: &'a str,
    /// The name to show.
    pub display_name: &'a str,
    /// The password.
    pub password: &'a str,
    /// Their address.
    pub email: Option<&'a str>,
}

/// What was given, checked and tidied: the name without the spaces around it
/// and the address without them.
#[derive(Debug, Clone)]
pub struct Clean {
    /// The username.
    pub username: String,
    /// The name to show.
    pub display_name: String,
    /// The address.
    pub email: Option<String>,
}

fn username_fits(name: &str) -> bool {
    let bytes = name.as_bytes();
    let plain = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    USERNAME_CHARS.contains(&bytes.len())
        && bytes.first().is_some_and(plain)
        && bytes
            .iter()
            .all(|byte| plain(byte) || *byte == b'_' || *byte == b'-')
}

/// Whether `text` can be an address: something before an `@` and a dotted
/// name after it, without spaces. Whether mail reaches it is for the mail
/// server to find out.
#[must_use]
pub fn email_fits(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    text.chars().count() <= EMAIL_MAX_CHARS
        && !local.is_empty()
        && domain.contains('.')
        && !domain.contains('@')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !text.chars().any(char::is_whitespace)
}

/// Checks what a person gave, all of it, and answers with every fault at
/// once. `email_needed` says that the system has to be able to reach them.
pub fn check(details: &Details<'_>, auth: &Auth, email_needed: bool) -> ApiResult<Clean> {
    let mut faults = Vec::new();
    let mut fault = |path: &str, message: &str| {
        faults.push(FieldFault {
            path: path.to_owned(),
            message: message.to_owned(),
        });
    };
    if !username_fits(details.username) {
        fault(
            "username",
            "must be 3 to 32 characters: lower-case letters, digits, - and _, beginning with a letter or a digit",
        );
    }
    let display_name = details.display_name.trim();
    if !DISPLAY_NAME_CHARS.contains(&display_name.chars().count()) {
        fault("display_name", "must be 1 to 64 characters");
    }
    let email = details
        .email
        .map(str::trim)
        .filter(|email| !email.is_empty());
    match email {
        Some(email) if !email_fits(email) => fault("email", "is not an address"),
        None if email_needed => fault("email", "is needed, to confirm the address"),
        _ => {}
    }
    // A common password alone is said by its own code; beside other faults it is one of them.
    let mut common = false;
    match passwords::check_new(
        details.password,
        auth.password_min_length,
        details.username,
        email,
    ) {
        Ok(()) => {}
        Err(damaged @ passwords::Refused::ListDamaged(_)) => {
            return Err(damaged.into_api_error("password"));
        }
        Err(refused) => {
            common = refused == passwords::Refused::Common;
            fault("password", &refused.message());
        }
    }
    if faults.is_empty() {
        Ok(Clean {
            username: details.username.to_owned(),
            display_name: display_name.to_owned(),
            email: email.map(str::to_owned),
        })
    } else if common && faults.len() == 1 {
        Err(passwords::Refused::Common.into_api_error("password"))
    } else {
        Err(Problem::invalid(faults).into())
    }
}

/// The language a new person starts with: the first of the two shipped that
/// their browser asks for, by the weights it gives and else in the order it
/// lists them, and English where it asks for neither.
#[must_use]
pub fn locale_of(headers: &http::HeaderMap) -> &'static str {
    let Some(asked) = headers
        .get(http::header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
    else {
        return "en";
    };
    let mut tags: Vec<(f32, usize, &str)> = asked
        .split(',')
        .enumerate()
        .filter_map(|(position, part)| {
            let mut pieces = part.split(';');
            let tag = pieces.next()?.trim();
            let weight = pieces
                .find_map(|piece| piece.trim().strip_prefix("q="))
                .and_then(|weight| weight.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            (weight > 0.0).then_some((weight, position, tag))
        })
        .collect();
    tags.sort_by(|first, second| second.0.total_cmp(&first.0).then(first.1.cmp(&second.1)));
    for (_, _, tag) in tags {
        match tag
            .split('-')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "zh" => return "zh-CN",
            "en" => return "en",
            _ => {}
        }
    }
    "en"
}

/// How a person is made.
#[derive(Debug, Clone)]
pub struct How {
    /// Whether the address counts as confirmed from the start.
    pub email_confirmed: bool,
    /// Whether they administer the system.
    pub is_system_admin: bool,
    /// The time zone the browser they register from gave, an IANA name.
    pub timezone: Option<String>,
}

/// A conflict in the public namespace, with the form field that can be corrected.
fn public_name_conflict() -> ApiError {
    ApiError::new(ErrorCode::ConflictExists, "that public name is taken").with(
        "errors",
        vec![FieldFault {
            path: "username".to_owned(),
            message: "is taken by a username or a root org".to_owned(),
        }],
    )
}

/// `409 conflict.exists` for a public name; other account conflicts disclose no address field.
pub fn taken(error: DbError) -> ApiError {
    match &error {
        DbError::Unique { constraint }
            if matches!(constraint.as_str(), "users_username_key" | "orgs_slug_key") =>
        {
            public_name_conflict()
        }
        DbError::Unique { .. } => ApiError::new(
            ErrorCode::ConflictExists,
            "the account change could not be completed",
        ),
        _ => ApiError::from(error),
    }
}

/// Whether the immutable public namespace is already occupied, including suspended people.
pub(crate) async fn public_name_taken(conn: &mut Conn, name: &str) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM public_names WHERE name = $1)")
            .bind(name)
            .fetch_one(conn)
            .await
            .map_err(DbError::from)?,
    )
}

/// Resolves optional personal-home creation for every human account supplier.
/// Only an administrator's account-creation form supplies per-home overrides.
pub(crate) async fn personal_org_start(
    conn: &mut Conn,
    said: Option<&orch_api_contract::admin::OrgLimitsChange>,
) -> ApiResult<Option<identity::PersonalOrg>> {
    let wanted = [
        keys::ORGS_PERSONAL_ENABLED,
        keys::ORGS_PERSONAL_CLOSED,
        keys::ORGS_PERSONAL_FLAT,
    ]
    .map(registry::get);
    let defs: Vec<_> = wanted.into_iter().flatten().collect();
    let snapshot = Snapshot::load(conn, Place::System, Wanted::Keys(&defs))
        .await
        .map_err(ApiError::internal)?;
    let policy =
        |key: &str| super::setting::<bool>(&snapshot, key).map(|value| value.unwrap_or(true));
    if !policy(keys::ORGS_PERSONAL_ENABLED)? {
        if said.is_some_and(|limits| limits.closed.is_some() || limits.flat.is_some()) {
            return Err(ApiError::invalid(
                "personal_org",
                "personal organization creation is disabled",
            ));
        }
        return Ok(None);
    }
    Ok(Some(identity::PersonalOrg {
        closed: said
            .and_then(|limits| limits.closed)
            .map_or_else(|| policy(keys::ORGS_PERSONAL_CLOSED), Ok)?,
        flat: said
            .and_then(|limits| limits.flat)
            .map_or_else(|| policy(keys::ORGS_PERSONAL_FLAT), Ok)?,
    }))
}

/// Makes the person in the caller's transaction, with the org that is theirs
/// and the audit row of their registration.
pub async fn make(
    ctx: &Ctx,
    conn: &mut Conn,
    clean: &Clean,
    password_hash: &str,
    how: &How,
) -> ApiResult<UserRow> {
    if how
        .timezone
        .as_deref()
        .is_some_and(|zone| zone.parse::<chrono_tz::Tz>().is_err())
    {
        return Err(ApiError::invalid(
            "timezone",
            "is not the name of a time zone",
        ));
    }
    if public_name_taken(conn, &clean.username).await? {
        return Err(public_name_conflict());
    }
    let personal_org = personal_org_start(conn, None).await?;

    let id = orch_types::new_id();
    let created = identity::create_user(
        conn,
        &identity::NewUser {
            id,
            username: &clean.username,
            display_name: &clean.display_name,
            email: clean.email.as_deref(),
            email_confirmed: how.email_confirmed,
            password_hash: Some(password_hash),
            locale: locale_of(&ctx.headers),
            is_system_admin: how.is_system_admin,
            created_by: None,
            personal_org,
        },
        ctx.now,
    )
    .await
    .map_err(taken)?;
    // A new person's zone follows the device they register from: the zone it gave, or UTC until a
    // browser of theirs reports one.
    identity::start_device_timezone(conn, id, how.timezone.as_deref(), ctx.now).await?;
    if let Some(org) = created.personal_org_id {
        orch_authz::pages::start(conn, StoredKind::Node, org, id, Time::new(ctx.now))
            .await
            .map_err(ApiError::internal)?;
    }
    record(
        ctx,
        conn,
        Actor::User(id),
        orch_db::audit::action!(AuditAction::UserRegistered),
        Some(id),
        &format!("{} was registered", clean.username),
        json!({ "system_admin": how.is_system_admin, "email_confirmed": how.email_confirmed }),
    )
    .await?;
    Ok(created.user)
}

/// Whether a public username or root-org slug is free; email is never a query key.
pub(crate) async fn name_availability(
    ctx: Ctx,
    query: orch_api_contract::auth::AuthNameAvailabilityGetQuery,
    (): (),
) -> ApiResult<orch_api_contract::auth::AuthNameAvailabilityGetReply> {
    if !username_fits(&query.name) {
        return Err(ApiError::invalid(
            "name",
            "must be a public username or short name, not an address",
        ));
    }
    let mut conn = ctx.app.db.acquire().await?;
    crate::sign_in::throttle::by_address(
        &ctx,
        &mut conn,
        &crate::sign_in::Limits::of(&ctx.app.config.auth),
        "name_availability",
    )
    .await?;
    let taken = public_name_taken(&mut conn, &query.name).await?;
    Ok(orch_api_contract::auth::AuthNameAvailabilityGetReply { available: !taken })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        Auth::default()
    }

    fn details<'a>(username: &'a str, password: &'a str, email: Option<&'a str>) -> Details<'a> {
        Details {
            username,
            display_name: "  Ada Lovelace ",
            password,
            email,
        }
    }

    fn faults(error: &ApiError) -> Vec<String> {
        error.problem.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|fault| fault["path"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn a_username_is_lower_case_letters_digits_hyphens_and_underscores() {
        for good in ["ada", "a1b", "0day", "ada_l-1", &"a".repeat(32)] {
            assert!(username_fits(good), "{good}");
        }
        for bad in [
            "ad",
            "Ada",
            "_ada",
            "-ada",
            "ada lovelace",
            "ada@x",
            "é-ada",
            &"a".repeat(33),
        ] {
            assert!(!username_fits(bad), "{bad}");
        }
    }

    #[test]
    fn what_a_person_gives_is_checked_all_at_once_and_tidied() {
        let clean = check(
            &details("ada", "0123456789", Some(" ada@example.org ")),
            &auth(),
            true,
        )
        .unwrap();
        assert_eq!(clean.display_name, "Ada Lovelace");
        assert_eq!(clean.email.as_deref(), Some("ada@example.org"));

        let refused = check(
            &details("Ada", "short", Some("not an address")),
            &auth(),
            true,
        )
        .unwrap_err();
        assert_eq!(faults(&refused), vec!["username", "email", "password"]);
        let no_address = check(&details("ada", "0123456789", None), &auth(), true).unwrap_err();
        assert_eq!(faults(&no_address), vec!["email"]);
        assert!(check(&details("ada", "0123456789", None), &auth(), false).is_ok());
        // A password that is the username is refused whatever else it is.
        let same = check(
            &details("ada-lovelace", "ada-lovelace", None),
            &auth(),
            false,
        )
        .unwrap_err();
        assert_eq!(faults(&same), vec!["password"]);
    }

    #[test]
    fn a_new_person_starts_in_the_language_their_browser_asks_for_among_the_two_shipped() {
        let asked = |value: &str| {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::ACCEPT_LANGUAGE, value.parse().unwrap());
            locale_of(&headers)
        };
        assert_eq!(asked("zh-CN,zh;q=0.9,en;q=0.8"), "zh-CN");
        assert_eq!(asked("en-GB, zh;q=0.5"), "en");
        assert_eq!(asked("en;q=0.4, zh-TW;q=0.9"), "zh-CN");
        assert_eq!(asked("fr, de;q=0.8, en;q=0.1"), "en");
        assert_eq!(asked("fr, de"), "en");
        assert_eq!(asked("zh;q=0, en"), "en");
        assert_eq!(locale_of(&http::HeaderMap::new()), "en");
    }

    #[test]
    fn an_address_has_something_before_the_at_and_a_dotted_name_after_it() {
        assert!(email_fits("ada@example.org"));
        assert!(email_fits("a.b+c@mail.example.org"));
        for bad in [
            "ada",
            "@example.org",
            "ada@example",
            "ada@.org",
            "ada@example.",
            "a b@example.org",
            "a@b@example.org",
        ] {
            assert!(!email_fits(bad), "{bad}");
        }
    }
}
