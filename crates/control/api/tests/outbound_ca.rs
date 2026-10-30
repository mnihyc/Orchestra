//! Stack-style certificate bundles add an operator authority without disabling verification or replacing public roots.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_webhook::WebhookNotifier;
use orch_pki::ca::{CertificateAuthority, Holder};
use orch_pki::certificates::ServiceName;
use orch_pki::verification::{TlsIdentity, plain_server_config};
use orch_types::{Clock, SystemClock, new_id};
use serde_json::json;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_rustls::TlsAcceptor;

// The parent blocks on a child process while the other runtime worker serves its TLS request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outgoing_https_accepts_the_operator_directory_and_keeps_the_bundled_roots() {
    let directory = tempfile::tempdir().unwrap();
    let public = directory.path().join("public.pem");
    let company = directory.path().join("company");
    let empty = directory.path().join("empty");
    std::fs::create_dir(&company).unwrap();
    std::fs::create_dir(&empty).unwrap();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let bundled = CertificateAuthority::create("Bundled test roots", 3650, clock.as_ref()).unwrap();
    let operator =
        CertificateAuthority::create("Company test authority", 3650, clock.as_ref()).unwrap();
    std::fs::write(&public, bundled.certificate_pem()).unwrap();
    std::fs::write(company.join("authority.pem"), operator.certificate_pem()).unwrap();
    assert!(
        Command::new("openssl")
            .arg("rehash")
            .arg(&company)
            .status()
            .unwrap()
            .success()
    );
    let (company_url, company_hits, company_task) = server(&operator, Arc::clone(&clock)).await;
    let (public_url, public_hits, public_task) = server(&bundled, clock).await;
    for (address, authority_dir, accepted) in [
        (&company_url, &company, true),
        (&company_url, &empty, false),
        (&public_url, &company, true),
    ] {
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "outgoing_ca_child", "--nocapture"])
            .env("ORCH_CA_TEST_URL", address)
            .env("ORCH_CA_TEST_ACCEPT", if accepted { "yes" } else { "no" })
            .env("SSL_CERT_FILE", &public)
            .env("SSL_CERT_DIR", authority_dir)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
    assert_eq!(
        company_hits.load(Ordering::SeqCst),
        1,
        "an untrusted issuer sends no webhook"
    );
    assert_eq!(
        public_hits.load(Ordering::SeqCst),
        1,
        "the added authority does not replace the bundle"
    );
    company_task.abort();
    public_task.abort();
}

#[test]
fn outgoing_ca_child() {
    let Ok(address) = std::env::var("ORCH_CA_TEST_URL") else {
        return;
    };
    let expected = std::env::var("ORCH_CA_TEST_ACCEPT").unwrap() == "yes";
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(async {
        WebhookNotifier::new().deliver(serde_json::from_value(json!({
            "id": new_id(), "attempt": 1,
            "config": {"values": {"url":address}, "secrets":{}, "allowed_private_hosts":["localhost"]},
            "to":{}, "recipient":null, "language":"en", "subject":null,
            "body":"{\"message\":\"test authority\"}", "notifications":[]
        })).unwrap()).await
    });
    if expected {
        result.unwrap();
    } else {
        let error = result.unwrap_err();
        assert!(
            matches!(error.code.as_str(), "unreachable" | "network"),
            "{error:?}"
        );
    }
}

async fn server(
    authority: &CertificateAuthority,
    clock: Arc<dyn Clock>,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let (key, certificate) = authority
        .issue_with_new_key(
            &Holder::Service {
                service: ServiceName::Api,
                names: vec!["localhost".to_owned()],
            },
            365,
            clock.as_ref(),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(
        plain_server_config(
            &TlsIdentity {
                chain: vec![certificate, authority.certificate().clone()],
                key,
            },
            clock,
        )
        .unwrap(),
    );
    let mut listener = None;
    for port in 41730..=41749 {
        if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            listener = Some(bound);
            break;
        }
    }
    let listener = listener.expect("one authority-test port is available");
    let address = format!(
        "https://localhost:{}/hook",
        listener.local_addr().unwrap().port()
    );
    let hits = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&hits);
    let task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let hits = Arc::clone(&observed);
            tokio::spawn(async move {
                let Ok(stream) = acceptor.accept(socket).await else {
                    return;
                };
                let handler = service_fn(move |request: Request<Incoming>| {
                    let hits = Arc::clone(&hits);
                    async move {
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        assert!(body.starts_with(b"{"));
                        hits.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(StatusCode::NO_CONTENT)
                                .body(Empty::<Bytes>::new())
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), handler)
                    .await;
            });
        }
    });
    (address, hits, task)
}
