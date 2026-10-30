//! Enrollment and renewal as the system judges them: once, in time, for the right authority, and with nothing left behind by a refusal.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_core::certificates::{
    Adopted, CertificateService, CertificateSettings, RenewCaller, TokenFor, TokenRequest,
};
use orch_config::ServerConfig;
use orch_db::Db;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::{hierarchy, resources};
use orch_link_contract::enrollment::{EnrollRequest, EnrollmentToken, RenewRequest};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_pki::ca::CertificateAuthority;
use orch_pki::certificates::Certificate;
use orch_pki::enrollment::PendingEnrollment;
use orch_pki::renewal;
use orch_testkit::clock;
use orch_testkit::database::TestDb;
use orch_types::{Id, ManualClock, new_id};
use std::sync::Arc;
use std::time::Duration;

struct Fixture {
    _test: TestDb,
    db: Db,
    clock: ManualClock,
    service: CertificateService,
    authority: Arc<CertificateAuthority>,
    user: Id,
}

async fn fixture() -> Fixture {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let clock = ManualClock::new(clock::start());
    let authority = Arc::new(CertificateAuthority::create("Test", 3_650, &clock).unwrap());
    let service = CertificateService::new(
        db.clone(),
        Arc::new(clock.clone()),
        Arc::clone(&authority),
        CertificateSettings::from_config(&ServerConfig::default()),
    );
    let user = {
        let mut conn = db.acquire().await.unwrap();
        // A real person has a personal root and its membership, including in identity tests.
        resources::testing::insert_person_with_home(&mut conn, "ada", clock::start())
            .await
            .unwrap()
    };
    Fixture {
        _test: test,
        db,
        clock,
        service,
        authority,
        user,
    }
}

fn request(token: &str, csr_pem: &str) -> EnrollRequest {
    EnrollRequest {
        machine_id: None,
        token: token.to_owned(),
        csr_pem: csr_pem.to_owned(),
        name: "alice-laptop".to_owned(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".to_owned(),
        protocol_versions: vec![1],
        roles: vec![Role::Agent],
    }
}

async fn machines(fixture: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM machines")
        .fetch_one(fixture.db.pool())
        .await
        .unwrap()
}

impl Fixture {
    async fn token(&self, purpose: TokenFor) -> String {
        self.service
            .issue_token(&TokenRequest {
                created_by: self.user,
                purpose,
                name_hint: None,
            })
            .await
            .unwrap()
            .token
    }

    fn csr() -> String {
        PendingEnrollment::start().unwrap().csr_pem().to_owned()
    }
}

#[tokio::test]
async fn a_token_enrolls_one_machine_that_belongs_to_whoever_made_the_token() {
    let fixture = fixture().await;
    let token = fixture.token(TokenFor::Machine).await;
    let reply = fixture
        .service
        .enroll(&request(&token, &Fixture::csr()), &Origin::default())
        .await
        .unwrap();
    let mut conn = fixture.db.acquire().await.unwrap();
    let row = resources::machine(&mut conn, reply.machine_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.owner_id, Some(fixture.user));
    assert_eq!(
        (row.placement.as_str(), row.name.as_str()),
        ("connected", "alice-laptop")
    );
    assert_eq!(row.roles, vec!["agent".to_owned()]);
    let certificate = Certificate::from_pem(&reply.certificate_pem).unwrap();
    assert_eq!(
        hex::encode(&row.cert_fingerprint),
        certificate.fingerprint()
    );
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'machine.enrolled' AND object_id = $1 AND actor_id = $2",
    )
    .bind(reply.machine_id)
    .bind(fixture.user)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        audited, 1,
        "the enrollment is one line of the audit trail, in the creator's name"
    );
    assert_eq!(
        reply.endpoints.len(),
        2,
        "and the machine is told where to connect"
    );

    // The same token again: it is gone.
    let again = fixture
        .service
        .enroll(&request(&token, &Fixture::csr()), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!((again.status, again.code), (410, "gone"));
    assert_eq!(machines(&fixture).await, 1);
}

#[tokio::test]
async fn a_token_that_ran_out_is_gone_and_one_that_never_existed_is_too() {
    let fixture = fixture().await;
    let token = fixture.token(TokenFor::Machine).await;
    fixture.clock.advance(chrono::Duration::minutes(16));
    let late = fixture
        .service
        .enroll(&request(&token, &Fixture::csr()), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(late.status, 410);

    let forged = EnrollmentToken::new("c2VjcmV0", fixture.authority.pin()).to_string();
    let unknown = fixture
        .service
        .enroll(&request(&forged, &Fixture::csr()), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(unknown.status, 410);
    assert_eq!(machines(&fixture).await, 0, "nobody was enrolled");
}

#[tokio::test]
async fn a_request_that_is_not_right_is_refused_without_using_the_token_up() {
    let fixture = fixture().await;
    let token = fixture.token(TokenFor::Machine).await;
    let parsed: EnrollmentToken = token.parse().unwrap();

    // A token that names another authority's pin.
    let other = CertificateAuthority::create("Other", 3_650, &fixture.clock).unwrap();
    let wrong_pin = EnrollmentToken::new(parsed.secret(), other.pin()).to_string();
    let refused = fixture
        .service
        .enroll(&request(&wrong_pin, &Fixture::csr()), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!((refused.status, refused.code), (400, "request.invalid"));

    // A request for a certificate that cannot be read.
    let refused = fixture
        .service
        .enroll(&request(&token, "garbage"), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(refused.status, 400);

    // A machine that speaks no protocol this system speaks.
    let mut future = request(&token, &Fixture::csr());
    future.protocol_versions = vec![9];
    let refused = fixture
        .service
        .enroll(&future, &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(
        (refused.status, refused.code),
        (409, "protocol_unsupported")
    );

    // A machine with no name and one with no role.
    let mut nameless = request(&token, &Fixture::csr());
    nameless.name = "  ".to_owned();
    assert_eq!(
        fixture
            .service
            .enroll(&nameless, &Origin::default())
            .await
            .unwrap_err()
            .status,
        400
    );
    assert_eq!(machines(&fixture).await, 0);

    // The token is still good.
    let reply = fixture
        .service
        .enroll(&request(&token, &Fixture::csr()), &Origin::default())
        .await;
    assert!(reply.is_ok(), "{:?}", reply.err());
}

#[tokio::test]
async fn two_machines_that_use_one_token_at_once_make_one() {
    let fixture = fixture().await;
    let token = fixture.token(TokenFor::Machine).await;
    let (first, second) = (
        request(&token, &Fixture::csr()),
        request(&token, &Fixture::csr()),
    );
    let origin = Origin::default();
    let (one, two) = tokio::join!(
        fixture.service.enroll(&first, &origin),
        fixture.service.enroll(&second, &origin)
    );
    assert_eq!(
        u8::from(one.is_ok()) + u8::from(two.is_ok()),
        1,
        "exactly one of them"
    );
    let refused = one.err().or(two.err()).unwrap();
    assert_eq!(refused.status, 410);
    assert_eq!(machines(&fixture).await, 1);
}

#[tokio::test]
async fn a_host_token_makes_a_host_of_the_system_or_of_an_org_with_its_root_found() {
    let fixture = fixture().await;
    // A host of the system.
    let token = fixture.token(TokenFor::Host { home: None }).await;
    let mut node = request(&token, &Fixture::csr());
    node.roles = vec![Role::Node];
    let reply = fixture
        .service
        .enroll(&node, &Origin::default())
        .await
        .unwrap();
    let (org, home, owner): (Option<Id>, Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT org_id, home_id, owner_id FROM hosts WHERE machine_id = $1")
            .bind(reply.machine_id)
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
    assert_eq!((org, home, owner), (None, None, Some(fixture.user)));
    let mut conn = fixture.db.acquire().await.unwrap();
    let row = resources::machine(&mut conn, reply.machine_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.placement.as_str(), row.owner_id),
        ("managed", Some(fixture.user)),
        "a host's machine is first owned by whoever enrolled it: only an internal runtime connector has none"
    );

    // A host whose home is an org inside an org: its org is the outermost.
    let (root, inner) = (new_id(), new_id());
    hierarchy::testing::insert_node(&mut conn, root, "org", None, "acme", fixture.user)
        .await
        .unwrap();
    hierarchy::testing::insert_node(&mut conn, inner, "org", Some(root), "team", fixture.user)
        .await
        .unwrap();
    // A host's first owner is a person of its root org: the member who made its token.
    orch_db::queries::access::testing::insert_member(&mut conn, root, fixture.user, inner)
        .await
        .unwrap();
    drop(conn);
    let token = fixture.token(TokenFor::Host { home: Some(inner) }).await;
    let mut node = request(&token, &Fixture::csr());
    node.roles = vec![Role::Node];
    let reply = fixture
        .service
        .enroll(&node, &Origin::default())
        .await
        .unwrap();
    let (org, home): (Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT org_id, home_id FROM hosts WHERE machine_id = $1")
            .bind(reply.machine_id)
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
    assert_eq!((org, home), (Some(root), Some(inner)));
}

/// What a host's page names, as (permission, holder kind, person): the rows its enrollment wrote.
async fn page_of_host(fixture: &Fixture, machine: Id) -> Vec<(String, String, Option<Id>)> {
    sqlx::query_as(
        "SELECT e.permission, e.holder_kind, e.holder_id
           FROM permission_entries e JOIN hosts h ON h.id = e.object_id
          WHERE e.object_kind = 'host' AND e.page = 'live' AND h.machine_id = $1
          ORDER BY e.permission",
    )
    .bind(machine)
    .fetch_all(fixture.db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_host_that_enrolls_has_a_page_with_its_enroller_first_and_one_of_the_system_is_open_to_everyone()
 {
    let fixture = fixture().await;
    let enrolled = |token: String| {
        let fixture = &fixture;
        async move {
            let mut node = request(&token, &Fixture::csr());
            node.roles = vec![Role::Node];
            fixture
                .service
                .enroll(&node, &Origin::default())
                .await
                .unwrap()
                .machine_id
        }
    };
    // A host of the system has no home whose people could be named: the Use row names everyone.
    let system = enrolled(fixture.token(TokenFor::Host { home: None }).await).await;
    let owner = (
        "access.own".to_owned(),
        "user".to_owned(),
        Some(fixture.user),
    );
    assert_eq!(
        page_of_host(&fixture, system).await,
        [
            owner.clone(),
            ("host.use".to_owned(), "everyone".to_owned(), None)
        ]
    );
    // A host of an org is its enroller's alone until its page, or the defaults of its home, say more.
    let org = new_id();
    {
        let mut conn = fixture.db.acquire().await.unwrap();
        hierarchy::testing::insert_node(&mut conn, org, "org", None, "acme", fixture.user)
            .await
            .unwrap();
        // A host's first owner is a person of its root org: the member who made its token.
        orch_db::queries::access::testing::insert_member(&mut conn, org, fixture.user, org)
            .await
            .unwrap();
    }
    let at_home = enrolled(fixture.token(TokenFor::Host { home: Some(org) }).await).await;
    assert_eq!(page_of_host(&fixture, at_home).await, [owner]);
    // A machine of a person is no host and has no page of one.
    let token = fixture.token(TokenFor::Machine).await;
    let plain = fixture
        .service
        .enroll(&request(&token, &Fixture::csr()), &Origin::default())
        .await
        .unwrap()
        .machine_id;
    assert!(page_of_host(&fixture, plain).await.is_empty());
}

/// A runtime of an environment on a host, as a host's node would be asked to make a unit for: the rows
/// that stand behind a token made for the connector in the unit.
async fn a_runtime(fixture: &Fixture) -> (Id, Id) {
    use orch_db::queries::environments::hosting::{
        NewManaged, NewRuntime, insert_managed, insert_runtime,
    };
    let mut conn = fixture.db.acquire().await.unwrap();
    let (org, host, template, environment, runtime) =
        (new_id(), new_id(), new_id(), new_id(), new_id());
    hierarchy::testing::insert_node(&mut conn, org, "org", None, "acme", fixture.user)
        .await
        .unwrap();
    let machine = resources::testing::insert_machine(&mut conn, "node", "managed")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO hosts (id, machine_id, owner_id, org_id, home_id, name) VALUES ($1, $2, $3, $4, $4, 'node')",
    )
    .bind(host)
    .bind(machine)
    .bind(fixture.user)
    .bind(org)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO env_templates (id, org_id, home_id, owner_id, name, kind, driver, spec)
         VALUES ($1, $2, $2, $3, 'Small', 'environment', 'docker', '{}')",
    )
    .bind(template)
    .bind(org)
    .bind(fixture.user)
    .execute(&mut *conn)
    .await
    .unwrap();
    insert_managed(
        &mut conn,
        &NewManaged {
            id: environment,
            name: "Main",
            owner_id: fixture.user,
            home_node_id: org,
            template_id: template,
            config: &serde_json::json!({}),
            host_entry_id: None,
            template_entry_id: None,
        },
        clock::start(),
    )
    .await
    .unwrap();
    insert_runtime(
        &mut conn,
        &NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: "orch-environment-000000000000",
            environment_id: environment,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &serde_json::json!({}),
        },
        clock::start(),
    )
    .await
    .unwrap();
    (host, runtime)
}

#[tokio::test]
async fn a_token_made_for_a_runtime_enrolls_a_managed_machine_that_the_runtime_knows_as_its_own_and_is_good_once()
 {
    let fixture = fixture().await;
    let (host, runtime) = a_runtime(&fixture).await;
    let token = fixture.token(TokenFor::Runtime { runtime }).await;
    let mut unit = request(&token, &Fixture::csr());
    unit.name = "orch-environment-000000000000".to_owned();
    unit.roles = vec![Role::Environment, Role::Agent];
    let reply = fixture
        .service
        .enroll(&unit, &Origin::default())
        .await
        .unwrap();
    let mut conn = fixture.db.acquire().await.unwrap();
    let row = resources::machine(&mut conn, reply.machine_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.placement.as_str(), row.owner_id),
        ("managed", None),
        "a unit the system keeps has no owner of its own"
    );
    assert_eq!(
        row.roles,
        vec!["environment".to_owned(), "agent".to_owned()]
    );
    let (of_host, of_runtime): (Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT host_id, runtime_id FROM machines WHERE id = $1")
            .bind(reply.machine_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!((of_host, of_runtime), (Some(host), Some(runtime)));
    let known: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        known,
        Some(reply.machine_id),
        "the runtime knows its machine"
    );
    let is_host: i64 = sqlx::query_scalar("SELECT count(*) FROM hosts WHERE machine_id = $1")
        .bind(reply.machine_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(is_host, 0, "a unit's connector is no host");
    drop(conn);
    // The token is single use.
    let again = fixture
        .service
        .enroll(&request(&token, &Fixture::csr()), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(again.status, 410);
    // A token for a runtime that is not there opens nothing, and leaves nothing behind.
    let before = machines(&fixture).await;
    let orphan = fixture.token(TokenFor::Runtime { runtime: new_id() }).await;
    let refused = fixture
        .service
        .enroll(&request(&orphan, &Fixture::csr()), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(refused.status, 410);
    assert_eq!(machines(&fixture).await, before);
}

#[tokio::test]
async fn a_renewal_is_refused_for_the_same_key_and_for_a_machine_that_was_revoked() {
    let fixture = fixture().await;
    let token = fixture.token(TokenFor::Machine).await;
    let pending = PendingEnrollment::start().unwrap();
    let csr = pending.csr_pem().to_owned();
    let enrolled = fixture
        .service
        .enroll(&request(&token, &csr), &Origin::default())
        .await
        .unwrap();
    let certificate = Certificate::from_pem(&enrolled.certificate_pem).unwrap();
    let caller = RenewCaller::Certificate(certificate.clone());

    let same_key = fixture
        .service
        .renew_machine(&caller, &RenewRequest { csr_pem: csr }, &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(same_key.status, 403, "a renewal has to use a new key");

    let (_, fresh) = renewal::start().unwrap();
    let reply = fixture
        .service
        .renew_machine(
            &caller,
            &RenewRequest {
                csr_pem: fresh.clone(),
            },
            &Origin::default(),
        )
        .await
        .unwrap();
    assert!(
        Certificate::from_pem(&reply.certificate_pem)
            .unwrap()
            .machine_id()
            .is_some()
    );

    assert!(
        fixture
            .service
            .revoke(
                enrolled.machine_id,
                Actor::User(fixture.user),
                &Origin::default()
            )
            .await
            .unwrap()
    );
    let (_, again) = renewal::start().unwrap();
    let refused = fixture
        .service
        .renew_machine(
            &caller,
            &RenewRequest { csr_pem: again },
            &Origin::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.status, 403, "a revoked machine renews nothing");
}

#[tokio::test]
async fn the_token_of_the_bundled_node_is_registered_in_the_name_of_the_first_administrator() {
    let fixture = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("node_enrollment_token");
    let issued =
        orch_pki::enrollment::issue_token(&fixture.authority, Duration::ZERO, &fixture.clock);
    std::fs::write(&file, format!("{}\n", issued.token)).unwrap();

    // At the first start nobody is an administrator yet.
    assert_eq!(
        fixture.service.adopt_token_file(&file).await.unwrap(),
        Adopted::WaitingForAdministrator
    );
    {
        let mut conn = fixture.db.acquire().await.unwrap();
        resources::testing::make_system_admin(&mut conn, fixture.user)
            .await
            .unwrap();
    }
    assert_eq!(
        fixture.service.adopt_token_file(&file).await.unwrap(),
        Adopted::Registered
    );
    assert_eq!(
        fixture.service.adopt_token_file(&file).await.unwrap(),
        Adopted::AlreadyKnown
    );

    // It enrolls a host, once, even a day later.
    fixture.clock.advance(chrono::Duration::hours(20));
    let mut node = request(&issued.token.to_string(), &Fixture::csr());
    node.roles = vec![Role::Node];
    node.name = "node".to_owned();
    let reply = fixture
        .service
        .enroll(&node, &Origin::default())
        .await
        .unwrap();
    let hosts: i64 = sqlx::query_scalar("SELECT count(*) FROM hosts WHERE machine_id = $1")
        .bind(reply.machine_id)
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
    assert_eq!(hosts, 1);
    assert_eq!(
        fixture.service.adopt_token_file(&file).await.unwrap(),
        Adopted::AlreadyKnown,
        "used, it is not made good again"
    );

    // A token file from another system's authority is refused.
    let other = CertificateAuthority::create("Other", 3_650, &fixture.clock).unwrap();
    let foreign = orch_pki::enrollment::issue_token(&other, Duration::ZERO, &fixture.clock);
    std::fs::write(&file, foreign.token.to_string()).unwrap();
    assert!(fixture.service.adopt_token_file(&file).await.is_err());
}
