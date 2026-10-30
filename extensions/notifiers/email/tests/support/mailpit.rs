//! A real SMTP server in Docker for tests, in every wire mode and with or without signing in.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of this")]

//! The server is Mailpit, in a container named `orch-test-smtp-…` that
//! listens on 127.0.0.1 at a port from 41000 to 41999 and is removed when the
//! value is dropped, a failed test included. Its messages are read back
//! through its HTTP interface, which listens on 127.0.0.1 as well.

use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The image, pinned to a release.
pub const IMAGE: &str = "axllent/mailpit:v1.31.2";

/// The ports tests may listen on.
const PORT_RANGE: (u16, u16) = (41_000, 42_000);

/// How the connection to the server is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// Plain text.
    Plain,
    /// Plain text that can be upgraded by STARTTLS.
    StartTls,
    /// TLS from the first byte.
    Tls,
}

/// The name and password the server asks for.
pub const LOGIN: (&str, &str) = ("orch", "s3cr3t-p4ss");

/// A certificate authority and a certificate for `localhost` and 127.0.0.1
/// that it signed, written where the container reads them.
pub struct Certificates {
    dir: tempfile::TempDir,
    /// The authority's certificate, PEM: what a channel names as `tls_ca`.
    pub ca_pem: String,
}

/// Makes an authority and a server certificate.
pub fn certificates() -> Certificates {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::default();
    ca.distinguished_name = DistinguishedName::new();
    ca.distinguished_name
        .push(DnType::CommonName, "orch test authority");
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca, ca_key);

    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf =
        CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()]).unwrap();
    leaf.distinguished_name = DistinguishedName::new();
    leaf.distinguished_name
        .push(DnType::CommonName, "localhost");
    leaf.is_ca = IsCa::ExplicitNoCa;
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer).unwrap();

    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "server.pem", &leaf_cert.pem());
    write(dir.path(), "server.key", &leaf_key.serialize_pem());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    Certificates {
        dir,
        ca_pem: ca_cert.pem(),
    }
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}

/// A running server.
pub struct Mailpit {
    name: String,
    /// Where it takes mail.
    pub smtp_port: u16,
    api_port: u16,
    /// What it reads its certificate from, where it has one.
    pub certificates: Option<Certificates>,
}

static STARTED: AtomicU32 = AtomicU32::new(0);

fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn some_port(seed: u64) -> u16 {
    let span = u64::from(PORT_RANGE.1 - PORT_RANGE.0);
    PORT_RANGE.0 + u16::try_from(seed % span).unwrap()
}

impl Mailpit {
    /// Starts a server with the wire mode and, where `login` is given, asking
    /// for that name and password.
    pub fn start(wire: Wire, login: Option<(&str, &str)>) -> Self {
        let certificates = (wire != Wire::Plain).then(certificates);
        let counter = u64::from(STARTED.fetch_add(1, Ordering::SeqCst));
        let clock = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut seed = (clock as u64) ^ (u64::from(std::process::id()) << 20) ^ (counter * 7_919);
        for attempt in 0..60 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let (smtp_port, api_port) = (some_port(seed >> 8), some_port(seed >> 24));
            if smtp_port == api_port || !port_free(smtp_port) || !port_free(api_port) {
                continue;
            }
            let name = format!("orch-test-smtp-w{}-{counter}-{attempt}", std::process::id());
            let mut command = Command::new("docker");
            command
                .args(["run", "-d", "--rm", "--name", &name])
                .args(["-p", &format!("127.0.0.1:{smtp_port}:1025")])
                .args(["-p", &format!("127.0.0.1:{api_port}:8025")]);
            if let Some(certs) = &certificates {
                command
                    .args(["-v", &format!("{}:/certs:ro", certs.dir.path().display())])
                    .args(["-e", "MP_SMTP_TLS_CERT=/certs/server.pem"])
                    .args(["-e", "MP_SMTP_TLS_KEY=/certs/server.key"]);
            }
            match wire {
                Wire::Plain => {}
                Wire::StartTls => {
                    command.args(["-e", "MP_SMTP_REQUIRE_STARTTLS=true"]);
                }
                Wire::Tls => {
                    command.args(["-e", "MP_SMTP_REQUIRE_TLS=true"]);
                }
            }
            if let Some((user, password)) = login {
                command.args(["-e", &format!("MP_SMTP_AUTH={user}:{password}")]);
                // The server refuses to be told both to require TLS and to
                // allow signing in without it: only a plain one is told the
                // second.
                if wire == Wire::Plain {
                    command.args(["-e", "MP_SMTP_AUTH_ALLOW_INSECURE=true"]);
                }
            }
            let output = command.arg(IMAGE).output().expect("docker can be run");
            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr);
                if error.contains("address already in use")
                    || error.contains("port is already allocated")
                    || error.contains("Conflict")
                {
                    continue;
                }
                panic!("docker could not start {IMAGE}: {error}");
            }
            let started = Self {
                name,
                smtp_port,
                api_port,
                certificates,
            };
            started.wait_until_ready();
            return started;
        }
        panic!("no free port from 41000 to 41999 was found for a test SMTP server");
    }

    fn wait_until_ready(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            let ready = Command::new("curl")
                .args(["-s", "-o", "/dev/null", "-f", "-m", "2"])
                .arg(format!("http://127.0.0.1:{}/api/v1/info", self.api_port))
                .status()
                .is_ok_and(|status| status.success());
            if ready {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("the test SMTP server {} did not come up", self.name);
    }

    /// The certificate authority's PEM, for a server that has a certificate.
    pub fn ca_pem(&self) -> Option<&str> {
        self.certificates
            .as_ref()
            .map(|certs| certs.ca_pem.as_str())
    }

    /// What the server has taken, the oldest first.
    pub async fn messages(&self) -> Vec<Received> {
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{}", self.api_port);
        let list: serde_json::Value = client
            .get(format!("{base}/api/v1/messages"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut ids: Vec<(String, String)> = list["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| {
                (
                    message["Created"].as_str().unwrap_or_default().to_owned(),
                    message["ID"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        ids.sort();
        let mut received = Vec::new();
        for (_, id) in ids {
            let one: serde_json::Value = client
                .get(format!("{base}/api/v1/message/{id}"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let headers: serde_json::Value = client
                .get(format!("{base}/api/v1/message/{id}/headers"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let addresses = |key: &str| -> Vec<String> {
                one[key]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|each| each["Address"].as_str().unwrap_or_default().to_owned())
                    .collect()
            };
            received.push(Received {
                subject: one["Subject"].as_str().unwrap_or_default().to_owned(),
                from: one["From"]["Address"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                to: addresses("To"),
                reply_to: addresses("ReplyTo"),
                text: one["Text"].as_str().unwrap_or_default().to_owned(),
                html: one["HTML"].as_str().unwrap_or_default().to_owned(),
                message_id: headers["Message-Id"][0]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        received
    }

    /// Waits until the server has taken `count` messages and returns them.
    pub async fn wait_for(&self, count: usize) -> Vec<Received> {
        for _ in 0..50 {
            let messages = self.messages().await;
            if messages.len() >= count {
                return messages;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the test SMTP server did not take {count} messages");
    }
}

impl Drop for Mailpit {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

/// A message the server took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// Its subject.
    pub subject: String,
    /// Who it is from.
    pub from: String,
    /// Whom it is to.
    pub to: Vec<String>,
    /// Where replies go.
    pub reply_to: Vec<String>,
    /// Its text part.
    pub text: String,
    /// Its HTML part.
    pub html: String,
    /// Its message id, with the angle brackets.
    pub message_id: String,
}
