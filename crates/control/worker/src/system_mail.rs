//! The mail that concerns a person's own user: what signing in writes into the outbox, the shipped text of the person's language, and the blanking of what must not stay once it is delivered.
//!
//! A confirmation of an address, the reset of a password, a code to sign in
//! with and the notice to an address that it was replaced are not
//! notifications: nobody chose to be told, and no route is on. Signing in writes
//! each as a row of the outbox that names the mail server it goes by, says
//! which message it is, to whom and with which values, and carries the same
//! message in English. The worker writes it in the language of the person
//! when it has the text for that language and otherwise sends what the row
//! carries.
//!
//! The row holds a secret: the link or the code that lets somebody in. It
//! has to be kept until the message is delivered, and not a moment after, so
//! once the message is delivered, or has failed for good, the secret and every
//! text that holds it are blanked.

use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use serde::Deserialize;

/// The values of `data` that hold a secret.
const SECRET_MEMBERS: [&str; 3] = ["token", "code", "url"];

/// A message that concerns a person's own user, as signing in wrote it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MailRow {
    /// Which message it is: `verify_email`, `reset_password`, `sign_in_code`
    /// or `email_changed`.
    pub purpose: String,
    /// The address it goes to.
    pub to: String,
    /// Whose it is.
    #[serde(default)]
    pub user_id: Option<Id>,
    /// The language they chose.
    #[serde(default)]
    pub language: String,
    /// The values the message is written from.
    #[serde(default)]
    pub data: Json,
    /// The message's subject in English.
    #[serde(default)]
    pub subject: String,
    /// The message in English.
    #[serde(default)]
    pub body: String,
}

impl MailRow {
    /// Reads the payload of an outbox row; none where it is not one of these.
    #[must_use]
    pub fn read(payload: &Json) -> Option<Self> {
        let row: Self = serde_json::from_value(payload.clone()).ok()?;
        (!row.purpose.is_empty() && !row.to.is_empty()).then_some(row)
    }

    fn text(&self, member: &str) -> Option<&str> {
        self.data
            .get(member)
            .and_then(Json::as_str)
            .filter(|text| !text.is_empty())
    }

    /// When the secret of the message stops being good, as it is written in a
    /// message.
    fn ends(&self) -> Option<String> {
        let at: DateTime<Utc> = self.text("expires_at")?.parse().ok()?;
        Some(at.format("%Y-%m-%d %H:%M UTC").to_string())
    }
}

/// A message as it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    /// The subject, one line.
    pub subject: String,
    /// The text.
    pub body: String,
}

/// Whether `language` is Chinese as the system writes it; every other is
/// English, the one text a row always carries.
fn is_chinese(language: &str) -> bool {
    let language = language.to_ascii_lowercase();
    language == "zh" || language.starts_with("zh-") || language.starts_with("zh_")
}

/// The message in the language of its person, from the shipped text of that
/// language, or else as the row carries it.
#[must_use]
pub fn text_of(row: &MailRow, language: &str, system: &str) -> Text {
    let own = || Text {
        subject: row.subject.clone(),
        body: row.body.clone(),
    };
    let ends = row.ends();
    let zh = is_chinese(language);
    let written = match (row.purpose.as_str(), row.text("url"), row.text("code"), row.text("new_email")) {
        ("verify_email", Some(url), _, _) => ends.as_deref().map(|ends| {
            if zh {
                (
                    format!("确认你在 {system} 的邮箱地址"),
                    format!("打开下面的链接以确认你的邮箱地址：\n{url}\n链接只能使用一次，有效期至 {ends}。"),
                )
            } else {
                (
                    format!("Confirm your address for {system}"),
                    format!("Open this link to confirm your address:\n{url}\nIt is good for one use, until {ends}."),
                )
            }
        }),
        ("reset_password", Some(url), _, _) => ends.as_deref().map(|ends| {
            if zh {
                (
                    format!("重置你在 {system} 的密码"),
                    format!("打开下面的链接以设置新密码：\n{url}\n链接只能使用一次，有效期至 {ends}。如果这不是你本人的操作，请忽略此邮件。"),
                )
            } else {
                (
                    format!("Reset your password for {system}"),
                    format!("Open this link to choose a new password:\n{url}\nIt is good for one use, until {ends}. If you did not ask for it, leave it unused."),
                )
            }
        }),
        ("sign_in_code", _, Some(code), _) => ends.as_deref().map(|ends| {
            if zh {
                (
                    format!("你在 {system} 的验证码：{code}"),
                    format!("你的登录验证码是 {code}。验证码只能使用一次，有效期至 {ends}。如果这不是你本人的操作，请忽略此邮件。"),
                )
            } else {
                (
                    format!("Your code for {system}: {code}"),
                    format!("Your code to sign in is {code}. It is good for one use, until {ends}. If you did not ask for it, leave it unused."),
                )
            }
        }),
        ("email_changed", _, _, Some(new_email)) => Some(if zh {
            (
                format!("你的 {system} 账号邮箱地址正在被更改"),
                format!("有人请求把你账号的邮箱地址更改为 {new_email}。新地址确认后才会生效。如果这不是你本人的操作，请修改你的密码。"),
            )
        } else {
            (
                format!("The address of your {system} account is being changed"),
                format!("Somebody asked for the address of your account to be changed to {new_email}. It changes when the new address is confirmed. If this was not you, change your password."),
            )
        }),
        _ => None,
    };
    match written {
        Some((subject, body)) => Text { subject, body },
        None => own(),
    }
}

/// The payload of a row once its message was delivered or failed for good:
/// the secret, the address that carries it and the texts that hold it are
/// blank. What the message was, to whom, and when it stopped being good stay,
/// for whoever looks into what was sent.
#[must_use]
pub fn scrubbed(payload: &Json) -> Json {
    let mut scrubbed = payload.clone();
    if let Some(row) = scrubbed.as_object_mut() {
        for member in ["subject", "body"] {
            if row.contains_key(member) {
                row.insert(member.to_owned(), Json::String(String::new()));
            }
        }
        if let Some(data) = row.get_mut("data").and_then(Json::as_object_mut) {
            for member in SECRET_MEMBERS {
                if data.contains_key(member) {
                    data.insert(member.to_owned(), Json::String(String::new()));
                }
            }
        }
        row.insert("blanked".to_owned(), Json::Bool(true));
    }
    scrubbed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(purpose: &str, data: Json) -> Json {
        json!({
            "purpose": purpose, "to": "ada@example.org", "user_id": "0199abcd-1234-7000-8000-0000000000b1",
            "language": "zh-CN", "data": data,
            "subject": "English subject", "body": "English body"
        })
    }

    #[test]
    fn a_row_that_signing_in_wrote_is_read_and_anything_else_is_not() {
        let payload = row("sign_in_code", json!({ "code": "ABCD-1234" }));
        let read = MailRow::read(&payload).unwrap();
        assert_eq!(
            (read.purpose.as_str(), read.to.as_str()),
            ("sign_in_code", "ada@example.org")
        );
        assert!(MailRow::read(&json!({ "notifications": [] })).is_none());
        assert!(MailRow::read(&json!({ "purpose": "x" })).is_none());
        assert!(MailRow::read(&Json::Null).is_none());
    }

    #[test]
    fn each_message_is_written_in_the_language_of_its_person_and_in_english_otherwise() {
        let ends = "2026-10-08T14:10:00.000Z";
        let cases = [
            (
                "verify_email",
                json!({ "url": "https://orch.example/verify-email/oem_x", "token": "oem_x", "expires_at": ends }),
            ),
            (
                "reset_password",
                json!({ "url": "https://orch.example/reset/oem_y", "token": "oem_y", "expires_at": ends }),
            ),
            (
                "sign_in_code",
                json!({ "code": "ABCD-1234", "expires_at": ends }),
            ),
            ("email_changed", json!({ "new_email": "new@example.org" })),
        ];
        for (purpose, data) in cases {
            let read = MailRow::read(&row(purpose, data)).unwrap();
            let chinese = text_of(&read, "zh-CN", "Acme Orch");
            let english = text_of(&read, "en", "Acme Orch");
            assert!(chinese.subject.contains("Acme Orch"), "{purpose}");
            assert!(
                chinese
                    .body
                    .chars()
                    .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "{purpose}: {}",
                chinese.body
            );
            assert!(
                english.subject.contains("Acme Orch") && english.body.is_ascii(),
                "{purpose}: {}",
                english.body
            );
            assert_ne!(chinese, english);
            assert!(!chinese.subject.contains('\n') && !english.subject.contains('\n'));
        }
        let code = MailRow::read(&row(
            "sign_in_code",
            json!({ "code": "ABCD-1234", "expires_at": ends }),
        ))
        .unwrap();
        let english = text_of(&code, "en", "Orch");
        assert_eq!(english.subject, "Your code for Orch: ABCD-1234");
        assert!(english.body.contains("until 2026-10-08 14:10 UTC"));
        assert_eq!(
            text_of(&code, "fr", "Orch"),
            english,
            "a language without a text of its own gets English"
        );
    }

    #[test]
    fn a_message_the_worker_cannot_write_is_sent_as_the_row_carries_it() {
        let unknown = MailRow::read(&row("invitation", json!({}))).unwrap();
        assert_eq!(
            text_of(&unknown, "zh-CN", "Orch"),
            Text {
                subject: "English subject".into(),
                body: "English body".into()
            }
        );
        let blanked = MailRow::read(&row("sign_in_code", json!({ "code": "" }))).unwrap();
        assert_eq!(
            text_of(&blanked, "en", "Orch").body,
            "English body",
            "without its code it is the row's own"
        );
    }

    #[test]
    fn once_delivered_the_secret_and_every_text_that_holds_it_are_blank_and_the_rest_stays() {
        let payload = row(
            "reset_password",
            json!({ "url": "https://orch.example/reset/oem_y", "token": "oem_y", "expires_at": "2026-10-08T14:10:00.000Z" }),
        );
        let blank = scrubbed(&payload);
        let text = blank.to_string();
        assert!(
            !text.contains("oem_y") && !text.contains("English"),
            "{text}"
        );
        assert_eq!(blank["purpose"], "reset_password");
        assert_eq!(blank["to"], "ada@example.org");
        assert_eq!(blank["data"]["expires_at"], "2026-10-08T14:10:00.000Z");
        assert_eq!(blank["data"]["token"], "");
        assert_eq!(blank["blanked"], true);
        assert_eq!(
            scrubbed(&scrubbed(&payload)),
            blank,
            "blanking twice changes nothing"
        );
    }
}
