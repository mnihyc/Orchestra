//! A mail server that plays its side of a conversation, signs in only the one who knows its password, refuses some recipients with its own words, and remembers what it was told.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// The name that signs in, and the only recipients refused: those whose
/// address begins `nobody@`.
pub const USER: &str = "orch";

/// The answer to a recipient the server does not know.
pub const NO_SUCH_MAILBOX: &str = "550 5.1.1 Recipient address rejected: User unknown";

/// A server on a loopback port.
pub struct Smtp {
    /// Its port.
    pub port: u16,
    told: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Smtp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Smtp {
    /// A server that signs in `orch` with `password` and takes mail for
    /// anybody but `nobody@…`. Without a password it offers no sign-in.
    pub async fn start(password: Option<&str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let told = Arc::new(Mutex::new(Vec::new()));
        let password = password.map(str::to_owned);
        let log = Arc::clone(&told);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(converse(stream, password.clone(), Arc::clone(&log)));
            }
        });
        Self { port, told, task }
    }

    /// What the server was told, line by line, and each message it took as one
    /// entry that begins `DATA:`.
    pub fn told(&self) -> Vec<String> {
        self.told.lock().unwrap().clone()
    }
}

async fn converse(
    stream: tokio::net::TcpStream,
    password: Option<String>,
    told: Arc<Mutex<Vec<String>>>,
) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    if write.write_all(b"220 double ESMTP\r\n").await.is_err() {
        return;
    }
    let mut message: Option<String> = None;
    let mut waiting_for_credentials = false;
    while let Ok(Some(line)) = lines.next_line().await {
        let reply = if let Some(text) = message.as_mut() {
            if line == "." {
                told.lock().unwrap().push(format!("DATA:{text}"));
                message = None;
                "250 2.0.0 queued as 42".to_owned()
            } else {
                text.push('\n');
                text.push_str(&line);
                continue;
            }
        } else if waiting_for_credentials {
            waiting_for_credentials = false;
            sign_in(&line, password.as_deref())
        } else {
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                if password.is_some() {
                    "250-double\r\n250-AUTH PLAIN\r\n250 8BITMIME".to_owned()
                } else {
                    "250-double\r\n250 8BITMIME".to_owned()
                }
            } else if let Some(credentials) = upper.strip_prefix("AUTH PLAIN") {
                if credentials.trim().is_empty() {
                    waiting_for_credentials = true;
                    "334 ".to_owned()
                } else {
                    sign_in(line["AUTH PLAIN".len()..].trim(), password.as_deref())
                }
            } else if upper.starts_with("MAIL") {
                told.lock().unwrap().push(line.clone());
                "250 2.1.0 ok".to_owned()
            } else if upper.starts_with("RCPT") {
                told.lock().unwrap().push(line.clone());
                if upper.contains("<NOBODY@") {
                    NO_SUCH_MAILBOX.to_owned()
                } else {
                    "250 2.1.5 ok".to_owned()
                }
            } else if upper.starts_with("DATA") {
                message = Some(String::new());
                "354 go ahead".to_owned()
            } else if upper.starts_with("QUIT") {
                let _ = write.write_all(b"221 bye\r\n").await;
                return;
            } else {
                "250 ok".to_owned()
            }
        };
        if write
            .write_all(format!("{reply}\r\n").as_bytes())
            .await
            .is_err()
        {
            return;
        }
    }
}

/// The answer to `AUTH PLAIN` with these credentials, which are the base64 of
/// an empty name, the user and the password, each after a zero byte.
fn sign_in(credentials: &str, password: Option<&str>) -> String {
    let decoded = STANDARD.decode(credentials.trim()).unwrap_or_default();
    let mut parts = decoded.split(|byte| *byte == 0);
    let (_, user, given) = (parts.next(), parts.next(), parts.next());
    let right = password.is_some_and(|password| {
        user == Some(USER.as_bytes()) && given == Some(password.as_bytes())
    });
    if right {
        "235 2.7.0 Authentication successful".to_owned()
    } else {
        "535 5.7.8 authentication credentials invalid".to_owned()
    }
}
