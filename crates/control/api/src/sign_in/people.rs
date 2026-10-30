//! A person as others see them and as they see themself: the avatar, what a mailed code may be for them, and the address they asked for and have not confirmed.
//!
//! Others read a [`UserRef`], which is the same wherever a person is named.
//! The person alone reads a [`Me`], which adds what they sign in with and how
//! they like the interface. What a mailed code may be for them is what every
//! root org they belong to allows, so that one way in cannot be used where an
//! org of theirs has shut it.

use crate::middleware::requests::{ApiError, ApiResult};
use chrono::{DateTime, Utc};
use orch_api_contract::users::{
    Avatar, Density, EmailCodeUse, Locale, Me, MeIdentity, Theme, TimezoneMode, UserKind, UserRef,
};
use orch_db::Conn;
use orch_db::queries::access;
use orch_db::queries::identity::{self, UserRow};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Time};
use serde::de::DeserializeOwned;
use unicode_segmentation::UnicodeSegmentation;

/// The letters an avatar shows: the first extended grapheme cluster of the
/// name, in its upper case where it has one.
#[must_use]
pub fn avatar_text(display_name: &str) -> String {
    display_name
        .graphemes(true)
        .next()
        .map(str::to_uppercase)
        .unwrap_or_default()
}

fn word<T: DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// The person as others see them.
#[must_use]
pub fn user_ref(row: &UserRow) -> UserRef {
    UserRef {
        id: row.id,
        kind: if row.kind == "service" {
            UserKind::Service
        } else {
            UserKind::Human
        },
        username: row.username.clone(),
        address: Some(format!("@{}", row.username)),
        display_name: row.display_name.clone(),
        avatar: Avatar {
            text: avatar_text(&row.display_name),
            color: u32::try_from(row.avatar_color).unwrap_or_default(),
        },
    }
}

/// What a mailed code may be for the person `user`: only what every root org
/// of theirs allows, their own left out, or the system's value where they
/// belong to no other; and nothing while they have no confirmed address.
/// Without a person it is the same reading for nobody, in the same
/// statements, so that a name that nobody has costs what any other does.
pub async fn email_code_uses(
    conn: &mut Conn,
    user: Option<&UserRow>,
) -> ApiResult<Vec<EmailCodeUse>> {
    let (id, personal, confirmed) = user.map_or((Id::nil(), None, false), |row| {
        (row.id, row.personal_org_id, row.email_confirmed())
    });
    let def = registry::get(keys::AUTH_EMAIL_CODE)
        .ok_or_else(|| ApiError::internal("the setting auth.email_code is not in the registry"))?;
    let orgs: Vec<Id> = access::memberships(conn, id)
        .await?
        .into_iter()
        .map(|membership| membership.org_id)
        .filter(|org| Some(*org) != personal)
        .collect();
    let places: Vec<Place> = if orgs.is_empty() {
        vec![Place::System]
    } else {
        orgs.into_iter().map(Place::Node).collect()
    };
    let mut allowed = vec![EmailCodeUse::SignIn, EmailCodeUse::SecondStep];
    for place in places {
        // The general settings loader also contains unrelated supplied-default futures.
        let snapshot = match Box::pin(Snapshot::load(conn, place, Wanted::Keys(&[def]))).await {
            Ok(snapshot) => snapshot,
            // An org that was deleted holds nobody to it.
            Err(orch_settings::Error::NotFound) => continue,
            Err(error) => return Err(ApiError::internal(error)),
        };
        let uses: Vec<EmailCodeUse> = snapshot
            .resolve(def)
            .value
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        allowed.retain(|each| uses.contains(each));
    }
    Ok(if confirmed { allowed } else { Vec::new() })
}

/// The person as they read themself, as of `now`.
pub async fn me(conn: &mut Conn, row: &UserRow, now: DateTime<Utc>) -> ApiResult<Me> {
    let email_pending = identity::pending_email(conn, row.id, now).await?;
    let email_code = email_code_uses(conn, Some(row)).await?;
    let (second_step_required, second_step_sources) = super::second_step::of_me(conn, row).await?;
    let identities = identity::identities_of(conn, row.id)
        .await?
        .into_iter()
        .map(|each| MeIdentity {
            idp_key: each.idp_key,
            idp_name: each.idp_name,
            email: each.email,
            linked_at: Time::new(each.linked_at),
        })
        .collect();
    Ok(Me {
        user: user_ref(row),
        email: row.email.clone(),
        email_verified: row.email_confirmed(),
        has_password: row.password_hash.is_some(),
        totp_enabled: row.totp_on(),
        email_pending,
        email_code,
        email_code_second_step: row.email_code_on(),
        second_step_required,
        second_step_sources,
        is_system_admin: row.is_system_admin,
        locale: word::<Locale>(&row.locale).unwrap_or(Locale::En),
        timezone: row.timezone.clone(),
        timezone_mode: word::<TimezoneMode>(&row.timezone_mode).unwrap_or_default(),
        theme: word::<Theme>(&row.theme).unwrap_or(Theme::System),
        density: word::<Density>(&row.density).unwrap_or(Density::Comfortable),
        personal_org_id: row.personal_org_id,
        created_at: Time::new(row.created_at),
        identities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_avatar_shows_the_first_letter_as_a_person_reads_it_in_its_upper_case() {
        assert_eq!(avatar_text("alice chen"), "A");
        assert_eq!(avatar_text("Éloïse"), "É");
        assert_eq!(avatar_text("陈小明"), "陈");
        // A letter with its accent in a second code point is still one letter.
        assert_eq!(avatar_text("e\u{301}cole"), "E\u{301}");
        // A flag is two code points and one letter to the eye.
        assert_eq!(
            avatar_text("\u{1F1E8}\u{1F1ED} Swiss"),
            "\u{1F1E8}\u{1F1ED}"
        );
        // A letter whose upper case is two letters shows both.
        assert_eq!(avatar_text("ßeta"), "SS");
        assert_eq!(avatar_text(""), "");
    }
}
