//! Configurable SMTP transport, authentication and TLS; and the check of a server when its channel is saved.
//!
//! A mail server is whatever speaks SMTP submission: its address and port,
//! how the connection is secured (`tls` from the first byte, `starttls`, or
//! `none`), how the client signs in (`plain`, `login`, whichever the server
//! offers, or not at all), and, for a server with a certificate from an
//! authority of its own, that authority's certificate. Nothing is tuned to a
//! provider.
//!
//! Before anything is sent the server's name is resolved here and every
//! address it has is held to the list of private hosts the channel is given;
//! the connection then goes to an address that was looked at, and the name is
//! used only to check the certificate. So a name that answers differently the
//! second time cannot lead a channel of an org or a person to a machine behind
//! the system.

use lettre::address::Address;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::{Credentials, DEFAULT_MECHANISMS, Mechanism};
use lettre::transport::smtp::client::{AsyncSmtpConnection, Certificate, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use orch_notifier_contract::delivery::{Config, DeliverError};
use orch_notifier_contract::outbound;
use orch_types::{Choice, Field, FieldType};
use std::net::SocketAddr;
use std::time::Duration;

/// The port of SMTP submission, which is where a mail server is when its
/// channel names no port.
pub const DEFAULT_PORT: u16 = 587;

/// How long one conversation with the server may take when the channel names
/// no time: long enough for a slow relay to take a message, short enough that
/// a server which stopped answering does not hold a delivery for minutes.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// The longest a channel may allow one conversation to take: the claim a
/// delivery holds on its row must outlast it with room to spare.
pub const MAX_TIMEOUT_SECS: u64 = 120;

/// How the connection to the server is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte, as on the port of SMTPS.
    Tls,
    /// A plain connection that is upgraded before anything is sent, and
    /// refused when the server cannot upgrade it.
    StartTls,
    /// No encryption: for a relay on a network that is trusted.
    None,
}

/// How the client signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// With the mechanism the server offers, if it has the client's name and
    /// password; without signing in if the channel has none.
    Auto,
    /// With `PLAIN`.
    Plain,
    /// With `LOGIN`.
    Login,
    /// Not at all, whatever the channel holds.
    None,
}

/// What is wrong with the values of a mail server's channel.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsFault {
    /// A value a server cannot be reached without is missing.
    #[error("{0} is required")]
    Missing(&'static str),
    /// A value cannot be read.
    #[error("{field} {problem}")]
    Invalid {
        /// The field.
        field: &'static str,
        /// What is wrong with it.
        problem: String,
    },
}

impl SettingsFault {
    /// The refusal that tells a person what to correct.
    #[must_use]
    pub fn refusal(&self) -> DeliverError {
        DeliverError::refused("invalid_config", &self.to_string())
    }
}

/// The values of a mail server's channel, read and checked.
#[derive(Clone)]
pub struct SmtpSettings {
    /// The server's name or address.
    pub host: String,
    /// Its port.
    pub port: u16,
    /// How the connection is secured.
    pub security: Security,
    /// How the client signs in.
    pub auth: Auth,
    /// The name it signs in with.
    pub username: Option<String>,
    /// Its password.
    pub password: Option<String>,
    /// Who the mail is from.
    pub from: Mailbox,
    /// Where replies go, where that is not the sender.
    pub reply_to: Option<Mailbox>,
    /// The name this client gives the server when it greets it.
    pub hello_name: Option<String>,
    /// The certificate of the authority of a server that has one of its own.
    pub tls_ca: Option<String>,
    /// How long one conversation may take.
    pub timeout: Duration,
}

impl std::fmt::Debug for SmtpSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpSettings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("auth", &self.auth)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "…"))
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

/// The fields a person fills in to set a mail server up.
#[must_use]
pub fn fields() -> Vec<Field> {
    let text = |name: &str, label: &str, required: bool, help: &str| {
        let mut field = Field::new(name, label, FieldType::Text);
        field.required = required;
        field.help = Some(help.to_owned());
        field
    };
    let choice = |name: &str, label: &str, help: &str, choices: &[(&str, &str)], default: &str| {
        let mut field = Field::new(name, label, FieldType::Choice);
        field.required = false;
        field.help = Some(help.to_owned());
        field.choices = Some(
            choices
                .iter()
                .map(|(value, label)| Choice {
                    value: (*value).to_owned(),
                    label: (*label).to_owned(),
                })
                .collect(),
        );
        field.default = Some(default.into());
        field
    };
    let mut port = text(
        "port",
        "Port",
        false,
        "Where the server takes mail to send: 587 for STARTTLS, 465 for TLS.",
    );
    port.default = Some(DEFAULT_PORT.to_string().into());
    let mut timeout = text(
        "timeout_secs",
        "Time allowed",
        false,
        "Seconds one conversation with the server may take.",
    );
    timeout.default = Some(DEFAULT_TIMEOUT_SECS.to_string().into());
    let mut password = Field::new("password", "Password", FieldType::Secret);
    password.required = false;
    let mut tls_ca = Field::new("tls_ca", "Certificate authority", FieldType::SecretText);
    tls_ca.required = false;
    tls_ca.help = Some(
        "The certificate, in PEM, of the authority that signed the server's, for a server whose certificate no public authority signed."
            .to_owned(),
    );
    vec![
        text(
            "host",
            "Server",
            true,
            "The name or address of the mail server.",
        ),
        port,
        choice(
            "security",
            "Security",
            "How the connection is secured.",
            &[
                ("starttls", "STARTTLS: upgraded before anything is sent"),
                ("tls", "TLS: encrypted from the first byte"),
                ("none", "None: for a relay on a trusted network"),
            ],
            "starttls",
        ),
        choice(
            "auth",
            "Sign-in",
            "How the client signs in to the server.",
            &[
                ("auto", "Whichever the server offers"),
                ("plain", "PLAIN"),
                ("login", "LOGIN"),
                ("none", "No sign-in"),
            ],
            "auto",
        ),
        text("username", "User name", false, "The name to sign in with."),
        password,
        text(
            "from",
            "Sender",
            true,
            "The address mail is sent from, as `Orch <orch@example.com>` or `orch@example.com`.",
        ),
        text(
            "reply_to",
            "Reply to",
            false,
            "Where replies go, where that is not the sender.",
        ),
        text(
            "hello_name",
            "Name for the greeting",
            false,
            "The name this system gives the server when it greets it; the name of the machine where that is left out.",
        ),
        tls_ca,
        timeout,
    ]
}

fn invalid(field: &'static str, problem: impl Into<String>) -> SettingsFault {
    SettingsFault::Invalid {
        field,
        problem: problem.into(),
    }
}

fn mailbox(field: &'static str, text: &str) -> Result<Mailbox, SettingsFault> {
    text.trim()
        .parse::<Mailbox>()
        .map_err(|_| invalid(field, "is not an address"))
}

impl SmtpSettings {
    /// Reads the values of a channel; the first thing wrong with them is
    /// returned.
    pub fn from_config(config: &Config) -> Result<Self, SettingsFault> {
        let host = config
            .text("host")
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .ok_or(SettingsFault::Missing("host"))?
            .to_owned();
        let port = match config
            .text("port")
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            None => DEFAULT_PORT,
            Some(text) => text
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .ok_or_else(|| invalid("port", "is not a port"))?,
        };
        let security = match config.text("security").map(str::trim) {
            None | Some("" | "starttls") => Security::StartTls,
            Some("tls") => Security::Tls,
            Some("none") => Security::None,
            Some(_) => return Err(invalid("security", "is one of starttls, tls and none")),
        };
        let auth = match config.text("auth").map(str::trim) {
            None | Some("" | "auto") => Auth::Auto,
            Some("plain") => Auth::Plain,
            Some("login") => Auth::Login,
            Some("none") => Auth::None,
            Some(_) => return Err(invalid("auth", "is one of auto, plain, login and none")),
        };
        let username = config
            .text("username")
            .map(str::to_owned)
            .filter(|name| !name.is_empty());
        let password = config
            .secret("password")
            .map(|secret| secret.expose().to_owned())
            .filter(|password| !password.is_empty());
        if matches!(auth, Auth::Plain | Auth::Login) && (username.is_none() || password.is_none()) {
            return Err(invalid("username", "and password are needed to sign in"));
        }
        let from = mailbox(
            "from",
            config.text("from").ok_or(SettingsFault::Missing("from"))?,
        )?;
        let reply_to = config
            .text("reply_to")
            .filter(|text| !text.trim().is_empty())
            .map(|text| mailbox("reply_to", text))
            .transpose()?;
        let hello_name = config
            .text("hello_name")
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        let tls_ca = config
            .secret("tls_ca")
            .map(|secret| secret.expose().trim().to_owned())
            .filter(|text| !text.is_empty());
        if let Some(pem) = &tls_ca {
            Certificate::from_pem(pem.as_bytes())
                .ok()
                .filter(|_| pem.contains("-----BEGIN CERTIFICATE-----"))
                .ok_or_else(|| invalid("tls_ca", "is not a certificate in PEM"))?;
        }
        let timeout = match config
            .text("timeout_secs")
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            None => DEFAULT_TIMEOUT_SECS,
            Some(text) => text
                .parse::<u64>()
                .ok()
                .filter(|secs| (1..=MAX_TIMEOUT_SECS).contains(secs))
                .ok_or_else(|| {
                    invalid(
                        "timeout_secs",
                        format!("is a number of seconds from 1 to {MAX_TIMEOUT_SECS}"),
                    )
                })?,
        };
        Ok(Self {
            host,
            port,
            security,
            auth,
            username,
            password,
            from,
            reply_to,
            hello_name,
            tls_ca,
            timeout: Duration::from_secs(timeout),
        })
    }

    /// The name this client greets the server with.
    fn hello(&self) -> ClientId {
        match &self.hello_name {
            Some(name) => ClientId::Domain(name.clone()),
            None => ClientId::default(),
        }
    }

    fn tls_parameters(&self) -> Result<TlsParameters, lettre::transport::smtp::Error> {
        let mut builder = TlsParameters::builder(self.host.clone());
        if let Some(pem) = &self.tls_ca {
            builder = builder.add_root_certificate(Certificate::from_pem(pem.as_bytes())?);
        }
        builder.build()
    }

    fn credentials(&self) -> Option<(Credentials, Vec<Mechanism>)> {
        let (user, password) = (self.username.clone()?, self.password.clone()?);
        let mechanisms = match self.auth {
            Auth::None => return None,
            Auth::Auto => DEFAULT_MECHANISMS.to_vec(),
            Auth::Plain => vec![Mechanism::Plain],
            Auth::Login => vec![Mechanism::Login],
        };
        Some((Credentials::new(user, password), mechanisms))
    }
}

/// What the conversation was doing when it failed, which decides what a
/// failure means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Looking at a server when its channel is saved.
    Check,
    /// Greeting a server, securing the connection and signing in, for a
    /// message about to be sent.
    Open,
    /// Handing a message over.
    Send,
}

/// The first status of the form `class.subject.detail` in a server's reply,
/// which says what is wrong more exactly than the three digits do.
fn enhanced_status(reply: &str) -> Option<(u8, u16, u16)> {
    reply.split_whitespace().find_map(|word| {
        let mut parts = word.trim_matches(|c: char| !c.is_ascii_digit()).split('.');
        let class = parts.next()?.parse::<u8>().ok()?;
        let subject = parts.next()?.parse::<u16>().ok()?;
        let detail = parts.next()?.parse::<u16>().ok()?;
        (parts.next().is_none() && matches!(class, 2 | 4 | 5)).then_some((class, subject, detail))
    })
}

/// Whether a permanent refusal says that the recipient's mailbox does not
/// exist or takes no mail, as opposed to something about the sender, the
/// message or the server: only that is the end of a route.
fn recipient_is_gone(reply: &str) -> bool {
    matches!(
        enhanced_status(reply),
        Some((5, 1, 1 | 2 | 3 | 6 | 10) | (5, 2, 1))
    )
}

/// Whether the failure is something the server said, in a reply with a code,
/// or the client's own refusal to go on.
fn replied_by_server(error: &lettre::transport::smtp::Error) -> bool {
    error.is_transient() || error.is_permanent() || error.is_client()
}

/// Whether the words of a failure that is no reply are those of a TLS
/// handshake that went wrong: a library reports a certificate that no
/// authority it knows signed as a failure to connect, in its own words.
fn says_tls(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "certificate",
        "handshake",
        "fatal alert",
        "peer is incompatible",
        "peer misbehaved",
        "tls",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

/// What a failure of the conversation means for the delivery.
#[must_use]
pub fn classify(error: &lettre::transport::smtp::Error, step: Step) -> DeliverError {
    let message = error
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let code = error
        .status()
        .map_or_else(|| "smtp".to_owned(), |code| code.to_string());
    if error.is_tls() || (!replied_by_server(error) && says_tls(&message)) {
        return if step == Step::Check {
            DeliverError::refused("tls", &message)
        } else {
            DeliverError::temporary("tls", &message)
        };
    }
    if error.is_timeout() {
        return DeliverError::temporary("timeout", &message);
    }
    if error.is_transient() {
        return DeliverError::temporary(&code, &message);
    }
    if error.is_permanent() {
        let auth = matches!(code.as_str(), "530" | "534" | "535" | "538")
            || (step != Step::Send && message.to_ascii_lowercase().contains("auth"));
        if auth {
            return DeliverError::refused("authentication_failed", &message);
        }
        if step == Step::Send && recipient_is_gone(&message) {
            return DeliverError::gone(&code, &message);
        }
        return DeliverError::refused(&code, &message);
    }
    if error.is_client() {
        return DeliverError::refused("unsupported", &message);
    }
    if error.is_response() {
        return if step == Step::Check {
            DeliverError::refused(
                "protocol",
                &format!(
                    "the server did not answer like an SMTP server; check the port and the security ({message})"
                ),
            )
        } else {
            DeliverError::temporary("protocol", &message)
        };
    }
    DeliverError::temporary("unreachable", &message)
}

/// The addresses of the server, each held to `allowed`: the ones a channel of
/// an org or a person may reach. None of them being allowed is a refusal.
async fn vetted_addresses(
    settings: &SmtpSettings,
    allowed: Option<&[String]>,
) -> Result<Vec<SocketAddr>, DeliverError> {
    let found: Vec<SocketAddr> = tokio::net::lookup_host((settings.host.as_str(), settings.port))
        .await
        .map_err(|error| {
            DeliverError::temporary(
                "unreachable",
                &format!("{} cannot be looked up: {error}", settings.host),
            )
        })?
        .collect();
    if found.is_empty() {
        return Err(DeliverError::temporary(
            "unreachable",
            &format!("{} has no address", settings.host),
        ));
    }
    let mut vetted = Vec::new();
    let mut refusal = None;
    for address in found {
        match outbound::may_connect(&settings.host, address.ip(), allowed) {
            Ok(()) => vetted.push(address),
            Err(refused) => refusal = Some(refused),
        }
    }
    match (vetted.is_empty(), refusal) {
        (true, Some(refused)) => Err(DeliverError::refused(
            "address_not_allowed",
            &refused.to_string(),
        )),
        _ => Ok(vetted),
    }
}

/// A conversation with the server that is ready for a message: connected,
/// secured and signed in.
pub struct Open {
    /// The connection.
    pub connection: AsyncSmtpConnection,
}

/// Connects to the server, secures the connection and signs in, within the
/// time the channel allows.
pub async fn open(
    settings: &SmtpSettings,
    allowed: Option<&[String]>,
    step: Step,
) -> Result<Open, DeliverError> {
    let attempt = async {
        let addresses = vetted_addresses(settings, allowed).await?;
        let hello = settings.hello();
        let tls = settings
            .tls_parameters()
            .map_err(|error| classify(&error, step))?;
        let mut last = DeliverError::temporary("unreachable", "no address answered");
        for address in addresses {
            let wrapper = (settings.security == Security::Tls).then(|| tls.clone());
            let connected = AsyncSmtpConnection::connect_tokio1(
                address,
                Some(settings.timeout),
                &hello,
                wrapper,
                None,
            )
            .await;
            let mut connection = match connected {
                Ok(connection) => connection,
                Err(error) => {
                    last = classify(&error, step);
                    continue;
                }
            };
            if settings.security == Security::StartTls
                && let Err(error) = connection.starttls(tls.clone(), &hello).await
            {
                return Err(classify(&error, step));
            }
            if let Some((credentials, mechanisms)) = settings.credentials()
                && let Err(error) = connection.auth(&mechanisms, &credentials).await
            {
                let mut fault = classify(&error, step);
                if error.is_permanent() || error.is_client() {
                    fault.code = "authentication_failed".to_owned();
                }
                return Err(fault);
            }
            return Ok(Open { connection });
        }
        Err(last)
    };
    match tokio::time::timeout(settings.timeout, attempt).await {
        Ok(result) => result,
        Err(_) => Err(DeliverError::temporary(
            "timeout",
            &format!(
                "{} did not answer in {} s",
                settings.host,
                settings.timeout.as_secs()
            ),
        )),
    }
}

/// Looks at the server: connects, secures the connection and signs in, and
/// leaves. Nothing is sent.
pub async fn check(
    settings: &SmtpSettings,
    allowed: Option<&[String]>,
) -> Result<(), DeliverError> {
    let mut open = open(settings, allowed, Step::Check).await?;
    let _ = tokio::time::timeout(settings.timeout, open.connection.quit()).await;
    Ok(())
}

/// The address a mailbox is.
#[must_use]
pub fn address_of(mailbox: &Mailbox) -> Address {
    mailbox.email.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_notifier_contract::delivery::Config;
    use orch_plugin_contract::Secret;
    use serde_json::json;

    fn config(values: serde_json::Value) -> Config {
        Config {
            values,
            ..Config::default()
        }
    }

    fn with_secret(mut config: Config, name: &str, value: &str) -> Config {
        config.secrets.insert(name.to_owned(), Secret::new(value));
        config
    }

    #[test]
    fn a_server_with_nothing_but_a_name_and_a_sender_takes_the_usual_settings() {
        let settings = SmtpSettings::from_config(&config(
            json!({ "host": "smtp.example.org", "from": "Orch <orch@example.org>" }),
        ))
        .unwrap();
        assert_eq!(settings.port, 587);
        assert_eq!(settings.security, Security::StartTls);
        assert_eq!(settings.auth, Auth::Auto);
        assert_eq!(settings.timeout, Duration::from_secs(30));
        assert!(
            settings.credentials().is_none(),
            "no name and password: no sign-in"
        );
    }

    #[test]
    fn every_value_of_a_server_is_read_and_a_wrong_one_names_itself() {
        let ok = with_secret(
            config(json!({
                "host": "mail.example.org", "port": "465", "security": "tls", "auth": "login",
                "username": "orch", "from": "orch@example.org", "reply_to": "Help <help@example.org>",
                "hello_name": "orch.example.org", "timeout_secs": "10"
            })),
            "password",
            "s3cr3t",
        );
        let settings = SmtpSettings::from_config(&ok).unwrap();
        assert_eq!(
            (settings.port, settings.security, settings.auth),
            (465, Security::Tls, Auth::Login)
        );
        let (_, mechanisms) = settings.credentials().unwrap();
        assert_eq!(mechanisms, [Mechanism::Login]);
        assert!(!format!("{settings:?}").contains("s3cr3t"));

        let fault = |values: serde_json::Value| {
            SmtpSettings::from_config(&config(values))
                .unwrap_err()
                .to_string()
        };
        assert_eq!(fault(json!({ "from": "a@b.c" })), "host is required");
        assert_eq!(fault(json!({ "host": "h" })), "from is required");
        assert!(fault(json!({ "host": "h", "from": "nonsense" })).starts_with("from "));
        assert!(
            fault(json!({ "host": "h", "from": "a@b.c", "port": "mail" })).starts_with("port ")
        );
        assert!(
            fault(json!({ "host": "h", "from": "a@b.c", "security": "ssl" }))
                .starts_with("security ")
        );
        assert!(
            fault(json!({ "host": "h", "from": "a@b.c", "auth": "plain" })).contains("password")
        );
        assert!(
            fault(json!({ "host": "h", "from": "a@b.c", "timeout_secs": "0" }))
                .starts_with("timeout_secs ")
        );
        assert!(
            fault(json!({ "host": "h", "from": "a@b.c", "timeout_secs": "9999" }))
                .starts_with("timeout_secs ")
        );
        let no_cert = with_secret(
            config(json!({ "host": "h", "from": "a@b.c" })),
            "tls_ca",
            "not a certificate",
        );
        assert!(
            SmtpSettings::from_config(&no_cert)
                .unwrap_err()
                .to_string()
                .starts_with("tls_ca ")
        );
    }

    #[test]
    fn a_server_that_is_told_not_to_sign_in_does_not_whatever_is_held() {
        let held = with_secret(
            config(json!({ "host": "h", "from": "a@b.c", "auth": "none", "username": "u" })),
            "password",
            "p",
        );
        assert!(
            SmtpSettings::from_config(&held)
                .unwrap()
                .credentials()
                .is_none()
        );
    }

    #[test]
    fn only_a_refusal_of_the_recipients_mailbox_ends_a_route() {
        assert!(recipient_is_gone(
            "permanent error (550): 5.1.1 <x@y.z>: Recipient address rejected: User unknown"
        ));
        assert!(recipient_is_gone(
            "permanent error (550): 5.2.1 mailbox disabled"
        ));
        assert!(
            !recipient_is_gone("permanent error (550): 5.7.1 relay access denied"),
            "that is about the sender"
        );
        assert!(!recipient_is_gone(
            "permanent error (554): 5.1.8 bad sender's system address"
        ));
        assert!(
            !recipient_is_gone("permanent error (550): no such user"),
            "without a status it is not sure"
        );
        assert_eq!(enhanced_status("550 5.1.1 nope"), Some((5, 1, 1)));
        assert_eq!(enhanced_status("version 1.2.3.4 is not a status"), None);
    }
}
