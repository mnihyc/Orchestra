//! Replacement enrollment tests against a real database and the session authorization path.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::*;
use crate::certificates::{RenewCaller, TokenFor, TokenRequest};
use crate::registry::Registry;
use crate::testing::TestApp;
use orch_db::queries::{hierarchy, resources};
use orch_link_contract::enrollment::RenewRequest;
use orch_link_contract::enrollment::{EnrollReply, EnrollRequest};
use orch_link_contract::session::{Arch, Os, Platform, TlsMode};
use orch_pki::certificates::Certificate;
use orch_pki::enrollment::PendingEnrollment;
use orch_testkit::{clock, database::TestDb};
use orch_types::new_id;

struct Office {
    _db: TestDb,
    app: TestApp,
    user: Id,
}

impl Office {
    async fn new() -> Self {
        let db = TestDb::fresh().await;
        let app = TestApp::new(
            Db::from_pool(db.pool.clone()),
            clock::manual(),
            Registry::new(),
        );
        let user = resources::testing::insert_person_with_home(
            &mut db.pool.acquire().await.unwrap(),
            "ada",
            app.app.clock.now(),
        )
        .await
        .unwrap();
        Self { _db: db, app, user }
    }

    async fn token(&self, purpose: TokenFor) -> crate::certificates::NewToken {
        self.app
            .app
            .link
            .certificates()
            .issue_token(&TokenRequest {
                created_by: self.user,
                purpose,
                name_hint: None,
            })
            .await
            .unwrap()
    }

    fn request(token: &str, name: &str) -> EnrollRequest {
        EnrollRequest {
            machine_id: None,
            token: token.into(),
            csr_pem: PendingEnrollment::start().unwrap().csr_pem().into(),
            name: name.into(),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            connector_version: "0.1.0".into(),
            protocol_versions: vec![1],
            roles: vec![Role::Environment],
        }
    }

    async fn enroll(&self, token: &str, name: &str) -> EnrollReply {
        self.app
            .app
            .link
            .certificates()
            .enroll(&Self::request(token, name), &Origin::default())
            .await
            .unwrap()
    }

    async fn authorize(&self, machine: Id, certificate: &str) -> AuthReply {
        self.app
            .app
            .link
            .authorize(&AuthQuery {
                endpoints: None,
                hosted_endpoints: None,
                n: 1,
                machine_id: machine,
                fingerprint: Certificate::from_pem(certificate).unwrap().fingerprint(),
                tls: TlsMode::Mutual,
                ip: "127.0.0.1".parse().unwrap(),
                connector_version: "0.1.0".into(),
                roles: vec![Role::Environment],
            })
            .await
    }
}

#[tokio::test]
async fn a_revoked_machine_enrolls_again_without_losing_its_name_owner_or_registered_environment() {
    // Keep this scope local: the other backplane proofs install their own capture subscriber.
    use tracing::instrument::WithSubscriber;
    async move {
        let office = Office::new().await;
        let first = office
            .enroll(&office.token(TokenFor::Machine).await.token, "laptop")
            .await;
        let root = new_id();
        let environment;
        {
            let mut conn = office.app.app.db.acquire().await.unwrap();
            hierarchy::testing::insert_node(&mut conn, root, "org", None, "Office", office.user)
                .await
                .unwrap();
            environment = resources::access_fixtures::insert_resource(
                &mut conn,
                "environment",
                Some(office.user),
                Some(root),
                "Work",
            )
            .await
            .unwrap();
            resources::access_fixtures::register_on(
                &mut conn,
                "environment",
                environment,
                first.machine_id,
            )
            .await
            .unwrap();
        }
        assert!(
            office
                .authorize(first.machine_id, &first.certificate_pem)
                .await
                .ok
        );
        office
            .app
            .app
            .link
            .revoke_machine(
                first.machine_id,
                Actor::User(office.user),
                &Origin::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            office
                .authorize(first.machine_id, &first.certificate_pem)
                .await
                .code,
            Some(RejectCode::Revoked)
        );
        let token = office
            .token(TokenFor::Again {
                machine: first.machine_id,
            })
            .await;
        let again = office
            .enroll(&token.token, "a different request name")
            .await;
        assert_eq!(again.machine_id, first.machine_id);
        assert_eq!(
            Certificate::from_pem(&again.certificate_pem)
                .unwrap()
                .machine_id(),
            Some(first.machine_id)
        );
        assert_eq!(
            office
                .authorize(first.machine_id, &first.certificate_pem)
                .await
                .code,
            Some(RejectCode::Revoked)
        );
        let renewed = office
            .authorize(again.machine_id, &again.certificate_pem)
            .await;
        assert!(
            renewed.ok,
            "the new certificate is let in: {:?}",
            renewed.code
        );
        assert_eq!(
            office
                .authorize(first.machine_id, &first.certificate_pem)
                .await
                .code,
            Some(RejectCode::Revoked)
        );
        let row = resources::machine(
            &mut office.app.app.db.acquire().await.unwrap(),
            first.machine_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(row.name, "laptop");
        assert_eq!(row.owner_id, Some(office.user));
        assert!(row.is_active());
        let registration: (Id, String) =
            sqlx::query_as("SELECT machine_id, name FROM environments WHERE id = $1")
                .bind(environment)
                .fetch_one(office.app.app.db.pool())
                .await
                .unwrap();
        assert_eq!(registration, (first.machine_id, "Work".into()));
        office.app.app.link.shutdown();
    }
    .with_subscriber(
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(tracing::Level::ERROR)
            .finish(),
    )
    .await
}

#[tokio::test]
async fn rejected_certificates_release_their_version_fence_before_authorization_answers() {
    let office = Office::new().await;
    let first = office
        .enroll(&office.token(TokenFor::Machine).await.token, "laptop")
        .await;
    // A separate already-open connection observes the fence without flushing the rejected
    // transaction by reusing its pooled connection. No transaction waits on another connection.
    let mut observer = office.app.app.db.acquire().await.unwrap();
    let mut query = AuthQuery {
        endpoints: None,
        hosted_endpoints: None,
        n: 1,
        machine_id: first.machine_id,
        fingerprint: "00".repeat(32),
        tls: TlsMode::Mutual,
        ip: "127.0.0.1".parse().unwrap(),
        connector_version: "0.1.0".into(),
        roles: vec![Role::Environment],
    };
    // Back-to-back rejections exercise deferred pool release as well as the inactive-row path.
    const REJECTIONS_PER_STATE: usize = 4;
    for revoked in [false, true] {
        if revoked {
            office
                .app
                .app
                .link
                .revoke_machine(
                    first.machine_id,
                    Actor::User(office.user),
                    &Origin::default(),
                )
                .await
                .unwrap();
            query.fingerprint = Certificate::from_pem(&first.certificate_pem)
                .unwrap()
                .fingerprint();
        }
        for _ in 0..REJECTIONS_PER_STATE {
            let refused = office.app.app.link.authorize(&query).await;
            assert!(!refused.ok);
            assert_eq!(refused.code, Some(RejectCode::Revoked));
            let released: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended('orch.component-update:' || ($1::uuid)::text, 0))")
                .bind(first.machine_id)
                .fetch_one(&mut *observer)
                .await
                .unwrap();
            assert!(
                released,
                "a rejected request must finish releasing its own fence before replying"
            );
        }
    }
    office.app.app.link.shutdown();
}

#[tokio::test]
async fn an_active_old_certificate_is_retired_when_the_replacement_first_opens_a_session() {
    let office = Office::new().await;
    let first = office
        .enroll(&office.token(TokenFor::Machine).await.token, "laptop")
        .await;
    let again = office
        .enroll(
            &office
                .token(TokenFor::Again {
                    machine: first.machine_id,
                })
                .await
                .token,
            "ignored",
        )
        .await;
    assert!(
        office
            .authorize(first.machine_id, &first.certificate_pem)
            .await
            .ok
    );
    assert!(
        office
            .authorize(first.machine_id, &again.certificate_pem)
            .await
            .ok
    );
    assert_eq!(
        office
            .authorize(first.machine_id, &first.certificate_pem)
            .await
            .code,
        Some(RejectCode::Revoked)
    );
    office.app.app.link.shutdown();
}

#[tokio::test]
async fn an_old_identity_cannot_renew_over_the_certificate_waiting_from_reenrollment() {
    let office = Office::new().await;
    let first = office
        .enroll(&office.token(TokenFor::Machine).await.token, "laptop")
        .await;
    let again = office
        .enroll(
            &office
                .token(TokenFor::Again {
                    machine: first.machine_id,
                })
                .await
                .token,
            "ignored",
        )
        .await;
    let csr = PendingEnrollment::start().unwrap().csr_pem().to_owned();
    let denied = office
        .app
        .app
        .link
        .certificates()
        .renew_machine(
            &RenewCaller::Certificate(Certificate::from_pem(&first.certificate_pem).unwrap()),
            &RenewRequest { csr_pem: csr },
            &Origin::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.status, 403);
    assert!(
        office
            .authorize(first.machine_id, &again.certificate_pem)
            .await
            .ok
    );
    assert_eq!(
        office
            .authorize(first.machine_id, &first.certificate_pem)
            .await
            .code,
        Some(RejectCode::Revoked)
    );
    office.app.app.link.shutdown();
}

#[tokio::test]
async fn a_replacement_token_cannot_identify_another_machine_and_is_good_once_and_only_in_time() {
    let office = Office::new().await;
    let first = office
        .enroll(&office.token(TokenFor::Machine).await.token, "first")
        .await;
    let other = office
        .enroll(&office.token(TokenFor::Machine).await.token, "second")
        .await;
    let token = office
        .token(TokenFor::Again {
            machine: first.machine_id,
        })
        .await;
    let request = Office::request(&token.token, "second");
    let again = office
        .app
        .app
        .link
        .certificates()
        .enroll(&request, &Origin::default())
        .await
        .unwrap();
    assert_eq!(again.machine_id, first.machine_id);
    assert_eq!(
        office
            .authorize(other.machine_id, &again.certificate_pem)
            .await
            .code,
        Some(RejectCode::Revoked)
    );
    let used = office
        .app
        .app
        .link
        .certificates()
        .enroll(&request, &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(used.status, 410);
    let late = office
        .token(TokenFor::Again {
            machine: first.machine_id,
        })
        .await;
    let validity =
        chrono::Duration::from_std(office.app.app.link.certificates().settings().token_ttl)
            .unwrap();
    office.app.clock.advance(validity);
    let expired = office
        .app
        .app
        .link
        .certificates()
        .enroll(&Office::request(&late.token, "first"), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(expired.status, 410);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM machines")
        .fetch_one(office.app.app.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 2);
    office.app.app.link.shutdown();
}

struct EnrollmentDesk(crate::certificates::CertificateService);

#[async_trait::async_trait]
impl orch_connector_core::enrollment::EnrollTransport for EnrollmentDesk {
    async fn enroll(
        &self,
        _: &str,
        _: &orch_link_contract::enrollment::EnrollmentToken,
        request: &EnrollRequest,
    ) -> Result<EnrollReply, orch_connector_core::enrollment::EnrollError> {
        self.0
            .enroll(request, &Origin::default())
            .await
            .map_err(|error| orch_connector_core::enrollment::EnrollError::Config(error.detail))
    }
    async fn renew(
        &self,
        _: &orch_link_contract::enrollment::Endpoint,
        _: &orch_pki::enrollment::MachineIdentity,
        _: &RenewRequest,
    ) -> Result<
        orch_link_contract::enrollment::RenewReply,
        orch_connector_core::enrollment::EnrollError,
    > {
        Err(orch_connector_core::enrollment::EnrollError::Unreachable(
            "this case requires explicit replacement".into(),
        ))
    }
}

#[tokio::test]
async fn retained_connector_and_node_directories_replace_only_their_bound_identity_and_keep_local_data()
 {
    use orch_connector_core::enrollment::{DataDir, EnrollInputs, enroll};
    for role in [Role::Environment, Role::Node] {
        for scenario in ["lapsed", "revoked", "disk-lost", "wrong-token"] {
            let office = Office::new().await;
            let desk = EnrollmentDesk(office.app.app.link.certificates().clone());
            let directory = tempfile::tempdir().unwrap();
            let data = DataDir::open(directory.path()).unwrap();
            let purpose = if role == Role::Node {
                TokenFor::Host { home: None }
            } else {
                TokenFor::Machine
            };
            let mut inputs = EnrollInputs {
                address: "wss://door.test:41702".into(),
                token: office
                    .token(if role == Role::Node {
                        TokenFor::Host { home: None }
                    } else {
                        TokenFor::Machine
                    })
                    .await
                    .token,
                name: "Kept machine".into(),
                roles: vec![role],
                platform: Platform {
                    os: Os::Linux,
                    arch: Arch::X86_64,
                },
                connector_version: "0.1.0".into(),
                managed: role == Role::Node,
            };
            let first = enroll(&inputs, &data, &desk, &office.app.clock)
                .await
                .unwrap();
            let before = first.identity.certificate.fingerprint();
            data.update_config(|config| {
                config.endpoints = vec![orch_config::connector::ConnectorEndpoint {
                    url: "wss://own-door.test:41703".into(),
                    trust: "pinned".into(),
                }];
                config.environment.remote_write = false;
                config.policy.delegated = Vec::new();
            })
            .unwrap();
            let config = std::fs::read(data.config_path()).unwrap();
            let registration = directory.path().join("state/registered-local-resource");
            std::fs::write(&registration, b"kept").unwrap();
            match scenario {
                "lapsed" => office.app.clock.advance(chrono::Duration::days(365 + 91)),
                "revoked" => {
                    office
                        .app
                        .app
                        .link
                        .revoke_machine(
                            first.record.machine_id,
                            Actor::User(office.user),
                            &Origin::default(),
                        )
                        .await
                        .unwrap();
                }
                _ => {}
            }
            inputs.name = "Name from installer must not rename it".into();
            inputs.token = office
                .token(if scenario == "wrong-token" {
                    purpose
                } else {
                    TokenFor::Again {
                        machine: first.record.machine_id,
                    }
                })
                .await
                .token;
            if scenario == "wrong-token" {
                let error = enroll(&inputs, &data, &desk, &office.app.clock)
                    .await
                    .err()
                    .unwrap();
                assert!(error.to_string().contains("Enroll again"), "{error}");
                assert_eq!(data.identity().unwrap().certificate.fingerprint(), before);
                assert_eq!(std::fs::read(data.config_path()).unwrap(), config);
                let fresh = tempfile::tempdir().unwrap();
                assert_ne!(
                    enroll(
                        &inputs,
                        &DataDir::open(fresh.path()).unwrap(),
                        &desk,
                        &office.app.clock
                    )
                    .await
                    .unwrap()
                    .record
                    .machine_id,
                    first.record.machine_id,
                    "the rejected token was not consumed"
                );
            } else {
                let lost = tempfile::tempdir().unwrap();
                let replacement = if scenario == "disk-lost" {
                    DataDir::open(lost.path()).unwrap()
                } else {
                    data.clone()
                };
                let next = enroll(&inputs, &replacement, &desk, &office.app.clock)
                    .await
                    .unwrap();
                assert_eq!(
                    next.record.machine_id, first.record.machine_id,
                    "{role:?} {scenario}"
                );
                assert_ne!(next.identity.certificate.fingerprint(), before);
                assert_eq!(std::fs::read(data.config_path()).unwrap(), config);
                assert_eq!(std::fs::read(&registration).unwrap(), b"kept");
                let auth = |certificate: &Certificate| AuthQuery {
                    n: 1,
                    machine_id: first.record.machine_id,
                    fingerprint: certificate.fingerprint(),
                    tls: TlsMode::Mutual,
                    ip: "127.0.0.1".parse().unwrap(),
                    connector_version: "0.1.0".into(),
                    roles: vec![role],
                    endpoints: None,
                    hosted_endpoints: None,
                };
                assert!(
                    office
                        .app
                        .app
                        .link
                        .authorize(&auth(&next.identity.certificate))
                        .await
                        .ok
                );
                assert_eq!(
                    office
                        .app
                        .app
                        .link
                        .authorize(&auth(&first.identity.certificate))
                        .await
                        .code,
                    Some(RejectCode::Revoked)
                );
                let row = resources::machine(
                    &mut office.app.app.db.acquire().await.unwrap(),
                    first.record.machine_id,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(row.name, "Kept machine");
            }
            office.app.app.link.shutdown();
        }
    }
}
