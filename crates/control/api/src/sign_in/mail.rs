//! The mail that concerns a person's own sign-in: the confirmation of an address, the reset of a password, a code to sign in with, and the notice to an address that it was replaced.
//!
//! Nothing is sent from a request. A message is a row of the outbox, written
//! in the transaction of the change that calls for it, for the worker to
//! deliver through the system's mail server and no other; where the system
//! has none, no row is written and the rules of the sign-in say what happens
//! instead. The row holds what the worker needs to write the message in the
//! person's language (which message it is, to whom, and its values) and the
//! message in English as a fallback.

use super::codes;
use crate::middleware::authentication::{new_token, token_hash};
use crate::middleware::requests::ApiResult;
use crate::registry::Ctx;
use chrono::{DateTime, Utc};
use orch_config::ServerConfig;
use orch_db::Conn;
use orch_db::queries::identity::{self, IssueCode, IssueLink, NewEmailToken};
use orch_types::{Id, Json, Time};
use serde_json::json;

/// The kind of channel that carries this mail.
pub const CHANNEL_KIND: &str = "email";

/// How long a link that confirms an address is good: a day, since mail may
/// wait in a box that is not looked at every hour.
pub const VERIFY_LINK_LIFE: chrono::Duration = chrono::Duration::hours(24);

/// How long a link that resets a password is good: half an hour, since it
/// opens the way in and is asked for by someone who is at the screen.
pub const RESET_LINK_LIFE: chrono::Duration = chrono::Duration::minutes(30);

/// Who a message is for.
#[derive(Debug, Clone, Copy)]
pub struct Addressee<'a> {
    /// The person.
    pub user_id: Id,
    /// The address it goes to.
    pub email: &'a str,
    /// The language they chose.
    pub locale: &'a str,
}

/// A message.
#[derive(Debug, Clone, Copy)]
pub enum Message<'a> {
    /// The link that confirms the address.
    VerifyEmail {
        /// The secret of the link.
        token: &'a str,
        /// When it stops being good.
        expires_at: DateTime<Utc>,
    },
    /// The link that sets a new password.
    ResetPassword {
        /// The secret of the link.
        token: &'a str,
        /// When it stops being good.
        expires_at: DateTime<Utc>,
    },
    /// The code that signs in, as it is shown: `XXXX-XXXX`.
    SignInCode {
        /// The code.
        code: &'a str,
        /// When it stops being good.
        expires_at: DateTime<Utc>,
    },
    /// The notice to an address that another is to take its place.
    AddressChanged {
        /// The address that is to replace it.
        new_email: &'a str,
    },
}

impl Message<'_> {
    /// Which message it is, as the worker names it.
    #[must_use]
    pub fn purpose(&self) -> &'static str {
        match self {
            Self::VerifyEmail { .. } => "verify_email",
            Self::ResetPassword { .. } => "reset_password",
            Self::SignInCode { .. } => "sign_in_code",
            Self::AddressChanged { .. } => "email_changed",
        }
    }
}

/// The address of the page that takes a link's secret: the origin a browser
/// opens, `/verify-email/<token>` or `/reset/<token>`.
#[must_use]
pub fn link(config: &ServerConfig, path: &str, token: &str) -> String {
    format!(
        "{}/{path}/{token}",
        config.system.public_url.trim_end_matches('/')
    )
}

/// The row for the outbox: the message, to whom, and the same in English.
#[must_use]
pub fn payload(config: &ServerConfig, to: &Addressee<'_>, message: &Message<'_>) -> Json {
    let system = &config.system.name;
    let (data, subject, body) = match message {
        Message::VerifyEmail { token, expires_at } => {
            let url = link(config, "verify-email", token);
            (
                json!({ "token": token, "url": url, "expires_at": Time::new(*expires_at) }),
                format!("Confirm your address for {system}"),
                format!(
                    "Open this link to confirm your address:\n{url}\nIt is good for one use, until {}.",
                    stamp(*expires_at)
                ),
            )
        }
        Message::ResetPassword { token, expires_at } => {
            let url = link(config, "reset", token);
            (
                json!({ "token": token, "url": url, "expires_at": Time::new(*expires_at) }),
                format!("Reset your password for {system}"),
                format!(
                    "Open this link to choose a new password:\n{url}\nIt is good for one use, until {}. If you did not ask for it, leave it unused.",
                    stamp(*expires_at)
                ),
            )
        }
        Message::SignInCode { code, expires_at } => (
            json!({ "code": code, "expires_at": Time::new(*expires_at) }),
            format!("Your code for {system}: {code}"),
            format!(
                "Your code to sign in is {code}. It is good for one use, until {}. If you did not ask for it, leave it unused.",
                stamp(*expires_at)
            ),
        ),
        Message::AddressChanged { new_email } => (
            json!({ "new_email": new_email }),
            format!("The address of your {system} account is being changed"),
            format!(
                "Somebody asked for the address of your account to be changed to {new_email}. It changes when the new address is confirmed. If this was not you, change your password."
            ),
        ),
    };
    json!({
        "purpose": message.purpose(),
        "to": to.email,
        "user_id": to.user_id,
        "language": to.locale,
        "data": data,
        "subject": subject,
        "body": body,
    })
}

fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// What an email token begins with.
pub const EMAIL_TOKEN_PREFIX: &str = "oem_";

/// What a link is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// To confirm an address.
    Verify,
    /// To set a new password.
    Reset,
}

impl Purpose {
    /// The word the link is stored with.
    #[must_use]
    pub fn stored(self) -> &'static str {
        match self {
            Self::Verify => "verify_email",
            Self::Reset => "reset_password",
        }
    }

    /// How long such a link is good.
    #[must_use]
    pub fn life(self) -> chrono::Duration {
        match self {
            Self::Verify => VERIFY_LINK_LIFE,
            Self::Reset => RESET_LINK_LIFE,
        }
    }
}

/// A link to mail.
#[derive(Debug, Clone, Copy)]
pub struct Link<'a> {
    /// What it is for.
    pub purpose: Purpose,
    /// Whom, and where.
    pub to: Addressee<'a>,
    /// Whether using it also switches the second step off.
    pub clear_second_step: bool,
}

/// Makes a link and queues the mail that carries it, in the transaction of
/// the caller, as one statement that does nothing unless `enabled`: a request
/// that answers every name alike does the same work for every name. Where the
/// system has no mail server (`channel`) nothing is written either. The answer
/// says whether anything was.
pub async fn send_link(
    ctx: &Ctx,
    conn: &mut Conn,
    channel: Option<Id>,
    link: &Link<'_>,
    enabled: bool,
) -> ApiResult<bool> {
    let token = new_token(EMAIL_TOKEN_PREFIX);
    let expires_at = ctx.now + link.purpose.life();
    let go = enabled && channel.is_some();
    let row = if go {
        let message = match link.purpose {
            Purpose::Verify => Message::VerifyEmail {
                token: &token,
                expires_at,
            },
            Purpose::Reset => Message::ResetPassword {
                token: &token,
                expires_at,
            },
        };
        payload(&ctx.app.config, &link.to, &message)
    } else {
        Json::Null
    };
    let written = identity::issue_link(
        conn,
        &IssueLink {
            token: NewEmailToken {
                id: orch_types::new_id(),
                user_id: link.to.user_id,
                purpose: link.purpose.stored(),
                token_hash: &token_hash(&token),
                email: link.to.email,
                clear_second_step: link.clear_second_step,
                at: ctx.now,
                expires_at,
            },
            channel: channel.unwrap_or_else(Id::nil),
            payload: &row,
        },
        go,
    )
    .await?;
    Ok(written)
}

/// Makes a code to sign in with, stores its hash for the person and queues
/// the mail that carries it, as [`send_link`] does a link. The answer says
/// whether anything was written.
pub async fn send_code(
    ctx: &Ctx,
    conn: &mut Conn,
    channel: Option<Id>,
    to: &Addressee<'_>,
    enabled: bool,
) -> ApiResult<bool> {
    let code = codes::new_code();
    let expires_at = ctx.now
        + chrono::Duration::seconds(
            i64::try_from(ctx.app.config.auth.email_code_ttl_secs).unwrap_or(i64::MAX),
        );
    let go = enabled && channel.is_some();
    let row = if go {
        payload(
            &ctx.app.config,
            to,
            &Message::SignInCode {
                code: &codes::display(&code),
                expires_at,
            },
        )
    } else {
        Json::Null
    };
    let written = identity::issue_code(
        conn,
        &IssueCode {
            user_id: to.user_id,
            code_hash: &codes::hash(to.user_id, &code),
            email: to.email,
            at: ctx.now,
            expires_at,
            channel: channel.unwrap_or_else(Id::nil),
            payload: &row,
        },
        go,
    )
    .await?;
    Ok(written)
}

/// Queues a message that has no link or code of its own, such as the notice
/// to an address that another is to take its place, where the system has a
/// mail server. The answer says whether it was queued.
pub async fn send_notice(
    ctx: &Ctx,
    conn: &mut Conn,
    channel: Option<Id>,
    to: &Addressee<'_>,
    message: &Message<'_>,
) -> ApiResult<bool> {
    let Some(channel) = channel else {
        return Ok(false);
    };
    let row = payload(&ctx.app.config, to, message);
    identity::queue_mail(conn, CHANNEL_KIND, channel, &row, ctx.now).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn config() -> ServerConfig {
        let mut config = ServerConfig::default();
        config.system.name = "Acme Orch".to_owned();
        config.system.public_url = "https://orch.acme.example/".to_owned();
        config
    }

    #[test]
    fn a_link_is_the_public_origin_the_page_and_the_secret() {
        assert_eq!(
            link(&config(), "reset", "oem_abc"),
            "https://orch.acme.example/reset/oem_abc"
        );
        assert_eq!(
            link(&config(), "verify-email", "oem_abc"),
            "https://orch.acme.example/verify-email/oem_abc"
        );
    }

    #[test]
    fn a_message_names_its_purpose_its_addressee_and_what_the_worker_needs() {
        let to = Addressee {
            user_id: orch_types::new_id(),
            email: "ada@example.org",
            locale: "zh-CN",
        };
        let ends = chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 10, 0)
            .unwrap();
        let row = payload(
            &config(),
            &to,
            &Message::SignInCode {
                code: "ABCD-1234",
                expires_at: ends,
            },
        );
        assert_eq!(row["purpose"], "sign_in_code");
        assert_eq!(row["to"], "ada@example.org");
        assert_eq!(row["language"], "zh-CN");
        assert_eq!(row["data"]["code"], "ABCD-1234");
        assert_eq!(row["data"]["expires_at"], "2026-10-08T14:10:00.000Z");
        assert!(row["body"].as_str().unwrap().contains("ABCD-1234"));

        let reset = payload(
            &config(),
            &to,
            &Message::ResetPassword {
                token: "oem_secret",
                expires_at: ends,
            },
        );
        assert_eq!(
            reset["data"]["url"],
            "https://orch.acme.example/reset/oem_secret"
        );
        assert!(reset["subject"].as_str().unwrap().contains("Acme Orch"));
    }
}
